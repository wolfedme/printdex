//! A backup repository: restic format through `rustic_core`.
//!
//! Every method blocks (rustic is synchronous), so the engine calls them
//! on a blocking thread. Long operations take a [`CancellationToken`] and
//! a progress callback: both are wired in through [`Guarded`], a backend
//! wrapper that fails the next read or write once the token is cancelled.
//! A cancelled backup leaves only unreferenced packs behind, which the
//! repository format tolerates (prune removes them).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use rustic_backend::local::LocalBackend;
use rustic_core::repofile::SnapshotFile;
use rustic_core::{
    BackupOptions, BytesList, CheckOptions, ConfigOptions, Credentials, ErrorKind, Excludes,
    FileType, Id, KeepOptions, KeyOptions, LocalDestination, LsOptions, OpenStatus, PathList,
    Progress, ProgressBars, ProgressType, PruneOptions, ReadBackend, ReadSubsetOption, Repository,
    RepositoryBackends, RepositoryOptions, RestoreOptions, RusticError, RusticProgress,
    RusticResult, SnapshotOptions, StringList, WriteBackend,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::remote::{RemoteBackend, Throttle};
use crate::error::{BackupError, CoreError, Result};

/// Where backups go. Holds no secrets (those are [`TargetSecrets`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Target {
    /// A local, removable or network-mounted folder.
    Folder {
        path: PathBuf,
    },
    /// Any S3-compatible service.
    S3 {
        endpoint: String,
        bucket: String,
        region: String,
        key_id: String,
        /// Folder inside the bucket ("" for the top).
        #[serde(default)]
        prefix: String,
    },
    WebDav {
        url: String,
        user: String,
    },
    /// SFTP with the system's ssh (Linux and macOS only).
    Sftp {
        host: String,
        port: u16,
        user: String,
        /// Folder on the server.
        path: String,
        /// Private key; the ssh agent and config are used when absent.
        key_file: Option<PathBuf>,
    },
}

impl Target {
    /// Whether the target is on this computer (folder) rather than across
    /// the network.
    pub fn is_folder(&self) -> bool {
        matches!(self, Target::Folder { .. })
    }
}

/// The secrets a target needs: the S3 secret key or WebDAV password, and
/// the repository passphrase ("" when encryption is off).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct TargetSecrets {
    pub secret: Option<String>,
    pub passphrase: String,
}

impl std::fmt::Debug for TargetSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TargetSecrets")
            .field("secret", &self.secret.as_ref().map(|_| "…"))
            .field("encrypted", &!self.passphrase.is_empty())
            .finish()
    }
}

/// How to open a repository.
#[derive(Clone, Debug, Default)]
pub struct RepoOptions {
    /// Local cache of index and tree files (saves downloads on remote
    /// targets); `None` runs without one.
    pub cache_dir: Option<PathBuf>,
    /// Upload limit in bytes per second, remote targets only.
    pub throttle: Option<u32>,
}

/// Bytes done and total of a running backup or restore.
pub type ProgressFn = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// One snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct SnapshotInfo {
    pub id: String,
    /// Unix milliseconds.
    pub time: i64,
    pub host: String,
    pub tags: Vec<String>,
    /// Files and bytes in the snapshot.
    pub files: u64,
    pub bytes: u64,
    /// New data this snapshot added to the repository (after dedup and
    /// compression).
    pub bytes_added: u64,
}

impl SnapshotInfo {
    fn from_file(s: &SnapshotFile) -> SnapshotInfo {
        let summary = s.summary.as_ref();
        SnapshotInfo {
            id: s.id.to_string(),
            time: s.time.timestamp().as_millisecond(),
            host: s.hostname.clone(),
            tags: s.tags.iter().cloned().collect(),
            files: summary.map_or(0, |m| m.total_files_processed),
            bytes: summary.map_or(0, |m| m.total_bytes_processed),
            bytes_added: summary.map_or(0, |m| m.data_added_packed),
        }
    }
}

/// File filter for a backup, in gitignore override syntax: `!pattern`
/// skips, a plain pattern includes, and the last matching pattern wins
/// (`iglobs` ignore case and come after `globs`). Empty backs up all.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Globs {
    pub globs: Vec<String>,
    pub iglobs: Vec<String>,
}

impl Globs {
    /// Skips files matching `patterns`.
    pub fn excluding(patterns: &[&str]) -> Globs {
        Globs {
            globs: patterns.iter().map(|p| format!("!{p}")).collect(),
            iglobs: Vec::new(),
        }
    }
}

/// What [`Repo::backup`] backs up.
#[derive(Clone, Copy, Debug)]
pub struct BackupJob<'a> {
    /// The folder to back up.
    pub source: &'a Path,
    /// Its path inside the snapshot, so a library keeps one history even
    /// when its folder moves.
    pub as_path: &'a str,
    /// Which files to back up (everything by default).
    pub filter: &'a Globs,
    pub tags: &'a [String],
    /// The install id of the computer backing up.
    pub host: &'a str,
}

/// Which snapshots to list (all of them when empty).
#[derive(Clone, Debug, Default)]
pub struct SnapshotFilter {
    pub host: Option<String>,
    pub tag: Option<String>,
}

impl SnapshotFilter {
    fn matches(&self, s: &SnapshotFile) -> bool {
        self.host.as_ref().is_none_or(|h| &s.hostname == h)
            && self
                .tag
                .as_ref()
                .is_none_or(|t| s.tags.iter().any(|x| x == t))
    }
}

/// How many snapshots to keep per host and tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    pub daily: u32,
    pub weekly: u32,
    pub monthly: u32,
}

/// The spec's retention: 7 daily, 4 weekly, 12 monthly.
pub const KEEP: Keep = Keep {
    daily: 7,
    weekly: 4,
    monthly: 12,
};

/// The cancel token and progress sink of the operation in flight.
#[derive(Default)]
struct Shared {
    cancel: Mutex<CancellationToken>,
    progress: Mutex<Option<ProgressFn>>,
}

impl Shared {
    fn cancelled(&self) -> bool {
        self.cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_cancelled()
    }

    fn begin(&self, cancel: &CancellationToken, progress: Option<ProgressFn>) {
        *self.cancel.lock().unwrap_or_else(PoisonError::into_inner) = cancel.clone();
        *self.progress.lock().unwrap_or_else(PoisonError::into_inner) = progress;
    }

    fn end(&self) {
        self.begin(&CancellationToken::new(), None);
    }

    fn check(&self) -> RusticResult<()> {
        if self.cancelled() {
            Err(RusticError::new(ErrorKind::Other, "cancelled"))
        } else {
            Ok(())
        }
    }
}

/// Passes everything to the real backend, failing once cancelled.
struct Guarded {
    inner: Arc<dyn WriteBackend>,
    shared: Arc<Shared>,
}

impl ReadBackend for Guarded {
    fn location(&self) -> String {
        self.inner.location()
    }
    fn list_with_size(&self, tpe: FileType) -> RusticResult<Vec<(Id, u32)>> {
        self.shared.check()?;
        self.inner.list_with_size(tpe)
    }
    fn list(&self, tpe: FileType) -> RusticResult<Vec<Id>> {
        self.shared.check()?;
        self.inner.list(tpe)
    }
    fn read_full(&self, tpe: FileType, id: &Id) -> RusticResult<Bytes> {
        self.shared.check()?;
        self.inner.read_full(tpe, id)
    }
    fn read_partial(
        &self,
        tpe: FileType,
        id: &Id,
        cacheable: bool,
        offset: u32,
        length: u32,
    ) -> RusticResult<Bytes> {
        self.shared.check()?;
        self.inner.read_partial(tpe, id, cacheable, offset, length)
    }
    fn warmup_path(&self, tpe: FileType, id: &Id) -> String {
        self.inner.warmup_path(tpe, id)
    }
}

impl WriteBackend for Guarded {
    fn create(&self) -> RusticResult<()> {
        self.shared.check()?;
        self.inner.create()
    }
    fn write_bytes(
        &self,
        tpe: FileType,
        id: &Id,
        cacheable: bool,
        content: BytesList,
    ) -> RusticResult<()> {
        self.shared.check()?;
        self.inner.write_bytes(tpe, id, cacheable, content)
    }
    fn remove(&self, tpe: FileType, id: &Id, cacheable: bool) -> RusticResult<()> {
        self.shared.check()?;
        self.inner.remove(tpe, id, cacheable)
    }
}

/// Sends rustic's byte progress (backup and restore) to the current sink.
#[derive(Clone)]
struct Bars(Arc<Shared>);

impl std::fmt::Debug for Bars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Bars")
    }
}

impl ProgressBars for Bars {
    fn progress(&self, kind: ProgressType, _prefix: &str) -> Progress {
        match kind {
            ProgressType::Bytes => Progress::new(ByteProgress {
                shared: self.0.clone(),
                state: Mutex::new((0, 0)),
            }),
            _ => Progress::hidden(),
        }
    }
}

#[derive(Debug)]
struct ByteProgress {
    shared: Arc<Shared>,
    /// Done, total.
    state: Mutex<(u64, u64)>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Shared")
    }
}

impl ByteProgress {
    fn report(&self, f: impl FnOnce(&mut (u64, u64))) {
        let (done, total) = {
            let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            f(&mut s);
            *s
        };
        let sink = self
            .shared
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sink) = sink {
            sink(done, total);
        }
    }
}

impl RusticProgress for ByteProgress {
    fn is_hidden(&self) -> bool {
        false
    }
    fn set_length(&self, len: u64) {
        self.report(|s| s.1 = len);
    }
    fn set_title(&self, _title: &str) {}
    fn inc(&self, inc: u64) {
        self.report(|s| s.0 += inc);
    }
    fn finish(&self) {}
}

/// An open repository.
pub struct Repo {
    /// Always `Some` between calls (indexing consumes and returns it).
    repo: Option<Repository<OpenStatus>>,
    be: Arc<dyn WriteBackend>,
    backends: RepositoryBackends,
    options: RepositoryOptions,
    shared: Arc<Shared>,
}

impl std::fmt::Debug for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Repo")
            .field("location", &self.be.location())
            .finish()
    }
}

/// The backend for a target, unwrapped.
fn backend(
    target: &Target,
    secrets: &TargetSecrets,
    opts: &RepoOptions,
) -> Result<Arc<dyn WriteBackend>> {
    if let Target::Folder { path } = target {
        let p = path.to_str().ok_or(BackupError::TargetInvalid)?;
        return Ok(Arc::new(LocalBackend::new(p, []).map_err(classify)?));
    }
    Ok(Arc::new(remote_backend(target, secrets, opts)?))
}

/// The OpenDAL backend for a remote target.
fn remote_backend(
    target: &Target,
    secrets: &TargetSecrets,
    opts: &RepoOptions,
) -> Result<RemoteBackend> {
    let throttle = opts.throttle.map(Throttle);
    let secret = secrets.secret.clone().unwrap_or_default();
    let be = match target {
        Target::Folder { .. } => return Err(BackupError::TargetInvalid.into()),
        Target::S3 {
            endpoint,
            bucket,
            region,
            key_id,
            prefix,
        } => {
            let b = opendal::services::S3::default()
                .endpoint(endpoint)
                .bucket(bucket)
                .region(region)
                .access_key_id(key_id)
                .secret_access_key(&secret)
                .root(&format!("/{}", prefix.trim_matches('/')))
                .disable_config_load()
                .disable_ec2_metadata();
            RemoteBackend::new(b, format!("s3:{endpoint}/{bucket}/{prefix}"), throttle)
                .map_err(classify)?
        }
        Target::WebDav { url, user } => {
            let b = opendal::services::Webdav::default()
                .endpoint(url)
                .username(user)
                .password(&secret);
            RemoteBackend::new(b, format!("webdav:{url}"), throttle).map_err(classify)?
        }
        #[cfg(unix)]
        Target::Sftp {
            host,
            port,
            user,
            path,
            key_file,
        } => {
            let mut b = opendal::services::Sftp::default()
                .endpoint(&format!("ssh://{host}:{port}"))
                .user(user)
                .root(path)
                .known_hosts_strategy("accept");
            if let Some(k) = key_file.as_ref().and_then(|k| k.to_str()) {
                b = b.key(k);
            }
            RemoteBackend::new(b, format!("sftp:{user}@{host}:{path}"), throttle)
                .map_err(classify)?
        }
        #[cfg(not(unix))]
        Target::Sftp { .. } => return Err(BackupError::Unsupported.into()),
    };
    Ok(be)
}

/// Writes and removes a small file at the target, to check it can be
/// reached and written.
pub(crate) fn probe(target: &Target, secrets: &TargetSecrets) -> Result<()> {
    const NAME: &str = ".printdex-probe";
    match target {
        Target::Folder { path } => {
            if !path.is_dir() {
                // A folder that doesn't exist yet is fine if it can be made.
                std::fs::create_dir_all(path).map_err(|e| classify_io(&e))?;
            }
            let file = path.join(NAME);
            std::fs::write(&file, b"printdex").map_err(|e| classify_io(&e))?;
            std::fs::remove_file(&file).map_err(|e| classify_io(&e))?;
            Ok(())
        }
        _ => {
            let be = remote_backend(target, secrets, &RepoOptions::default())?;
            be.probe(NAME).map_err(classify)
        }
    }
}

impl Repo {
    /// Opens the repository at `target`, creating it if the target has
    /// none yet (zstd compression, restic format version 2).
    pub fn open_or_init(
        target: &Target,
        secrets: &TargetSecrets,
        opts: &RepoOptions,
    ) -> Result<Repo> {
        Repo::connect(target, secrets, opts, true)
    }

    /// Opens an existing repository (restore never creates one).
    pub fn open(target: &Target, secrets: &TargetSecrets, opts: &RepoOptions) -> Result<Repo> {
        Repo::connect(target, secrets, opts, false)
    }

    fn connect(
        target: &Target,
        secrets: &TargetSecrets,
        opts: &RepoOptions,
        init: bool,
    ) -> Result<Repo> {
        if let Target::Folder { path } = target
            && init
        {
            std::fs::create_dir_all(path).map_err(|e| classify_io(&e))?;
        }
        let inner = backend(target, secrets, opts)?;
        let shared = Arc::new(Shared::default());
        let be: Arc<dyn WriteBackend> = Arc::new(Guarded {
            inner,
            shared: shared.clone(),
        });
        let backends = RepositoryBackends::new(be.clone(), None);
        let mut ropts = RepositoryOptions::default();
        match &opts.cache_dir {
            Some(dir) => ropts.cache_dir = Some(dir.clone()),
            None => ropts.no_cache = true,
        }
        let fresh = Repository::new_with_progress(&ropts, &backends, Bars(shared.clone()))
            .map_err(classify)?;
        let creds = Credentials::password(&secrets.passphrase);
        let exists = fresh.config_id().map_err(classify)?.is_some();
        let repo = if exists {
            fresh.open(&creds).map_err(classify)?
        } else if init {
            fresh
                .init(&creds, &KeyOptions::default(), &ConfigOptions::default())
                .map_err(classify)?
        } else {
            return Err(BackupError::NoRepository.into());
        };
        Ok(Repo {
            repo: Some(repo),
            be,
            backends,
            options: ropts,
            shared,
        })
    }

    fn repo(&self) -> Result<&Repository<OpenStatus>> {
        self.repo
            .as_ref()
            .ok_or_else(|| BackupError::Repository("closed after an error".into()).into())
    }

    fn take(&mut self) -> RusticResult<Repository<OpenStatus>> {
        self.repo
            .take()
            .ok_or_else(|| RusticError::new(ErrorKind::Other, "repository closed after an error"))
    }

    /// Runs `f` with the token and progress sink installed, mapping a
    /// cancelled run to [`CoreError::Cancelled`].
    fn guarded<T>(
        &mut self,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
        f: impl FnOnce(&mut Self) -> RusticResult<T>,
    ) -> Result<T> {
        self.shared.begin(cancel, progress);
        let out = f(self);
        let cancelled = self.shared.cancelled();
        self.shared.end();
        match out {
            Ok(v) => Ok(v),
            Err(_) if cancelled => Err(CoreError::Cancelled),
            Err(e) => Err(classify(e)),
        }
    }

    /// Backs up one folder (see [`BackupJob`]).
    pub fn backup(
        &mut self,
        job: &BackupJob<'_>,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<SnapshotInfo> {
        let BackupJob {
            source,
            as_path,
            filter,
            tags,
            host,
        } = *job;
        let src = source.to_str().ok_or(BackupError::TargetInvalid)?;
        let source = PathList::from_string(src)
            .map_err(classify)?
            .sanitize()
            .map_err(|_| BackupError::TargetInvalid)?;
        let mut snap = SnapshotOptions::default()
            .host(host.to_owned())
            .to_snapshot()
            .map_err(classify)?;
        if !tags.is_empty() {
            let list: StringList = tags
                .join(",")
                .parse()
                .map_err(|_| BackupError::TargetInvalid)?;
            let _ = snap.add_tags(vec![list]);
        }
        let mut opts = BackupOptions::default().as_path(PathBuf::from(as_path));
        opts.excludes = Excludes::default()
            .globs(filter.globs.clone())
            .iglobs(filter.iglobs.clone());
        self.guarded(cancel, progress, move |me| {
            let repo = me.take()?;
            let indexed = repo.to_indexed_ids()?;
            let out = indexed.backup(&opts, &source, snap);
            me.repo = Some(indexed.drop_index());
            out.map(|s| SnapshotInfo::from_file(&s))
        })
    }

    /// Snapshots matching `filter`, oldest first.
    pub fn snapshots(&self, filter: &SnapshotFilter) -> Result<Vec<SnapshotInfo>> {
        let mut out: Vec<_> = self
            .repo()?
            .get_all_snapshots()
            .map_err(classify)?
            .iter()
            .filter(|s| filter.matches(s))
            .map(SnapshotInfo::from_file)
            .collect();
        out.sort_by_key(|s| s.time);
        Ok(out)
    }

    /// Restores `subpath` of a snapshot (relative to the backed-up folder;
    /// "" for all of it) into `dest`, which is created if needed.
    pub fn restore(
        &mut self,
        snapshot: &str,
        subpath: &str,
        dest: &Path,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<()> {
        let snap = self
            .repo()?
            .get_snapshot_from_str(snapshot, |_| true)
            .map_err(|_| BackupError::SnapshotNotFound)?;
        let root = snap
            .paths
            .iter()
            .next()
            .cloned()
            .ok_or(BackupError::SnapshotNotFound)?;
        let inner = Path::new(&root).join(subpath);
        let inner = inner.to_string_lossy().replace('\\', "/");
        std::fs::create_dir_all(dest).map_err(|e| classify_io(&e))?;
        let dest_str = format!("{}/", dest.to_string_lossy().trim_end_matches(['/', '\\']));
        self.guarded(cancel, progress, move |me| {
            let repo = me.take()?;
            let indexed = repo.to_indexed()?;
            let out = (|| {
                let node = indexed.node_from_snapshot_and_path(&snap, &inner)?;
                let ls = LsOptions::default().recursive(true);
                let dest = LocalDestination::new(&dest_str, true, !node.is_dir())?;
                let opts = RestoreOptions::default();
                let plan = indexed.prepare_restore(&opts, indexed.ls(&node, &ls)?, &dest, false)?;
                indexed.restore(plan, &opts, indexed.ls(&node, &ls)?, &dest)
            })();
            me.repo = Some(indexed.drop_index());
            out
        })
    }

    /// Thins the snapshots of one `host` and `tag` to `keep`; other hosts'
    /// and tags' snapshots are never touched. Returns how many it removed.
    pub fn forget(&mut self, keep: Keep, host: &str, tag: &str) -> Result<u32> {
        let filter = SnapshotFilter {
            host: Some(host.to_owned()),
            tag: Some(tag.to_owned()),
        };
        let snaps: Vec<SnapshotFile> = self
            .repo()?
            .get_all_snapshots()
            .map_err(classify)?
            .into_iter()
            .filter(|s| filter.matches(s))
            .collect();
        let opts = KeepOptions::default()
            .keep_daily(i32::try_from(keep.daily).unwrap_or(i32::MAX))
            .keep_weekly(i32::try_from(keep.weekly).unwrap_or(i32::MAX))
            .keep_monthly(i32::try_from(keep.monthly).unwrap_or(i32::MAX));
        let now = rustic_core::jiff::Zoned::now();
        let ids: Vec<_> = opts
            .apply(snaps, &now)
            .map_err(classify)?
            .into_iter()
            .filter(|s| !s.keep)
            .map(|s| s.snapshot.id)
            .collect();
        if !ids.is_empty() {
            self.repo()?.delete_snapshots(&ids).map_err(classify)?;
        }
        Ok(u32::try_from(ids.len()).unwrap_or(u32::MAX))
    }

    /// Removes data no snapshot uses any more. Returns the bytes freed.
    pub fn prune(&mut self) -> Result<u64> {
        let before = self.size()?;
        let opts = PruneOptions::default();
        let plan = self.repo()?.prune_plan(&opts).map_err(classify)?;
        self.repo()?.prune(&opts, plan).map_err(classify)?;
        Ok(before.saturating_sub(self.size()?))
    }

    /// Checks the repository's structure and reads `read_percent` % of the
    /// stored data back.
    pub fn check(&mut self, read_percent: f64) -> Result<()> {
        let opts = CheckOptions::default()
            .read_data(read_percent > 0.0)
            .read_data_subset(ReadSubsetOption::Percentage(read_percent));
        let results = self.repo()?.check(opts).map_err(classify)?;
        results
            .is_ok()
            .map_err(|e| BackupError::CheckFailed(e.to_string()).into())
    }

    /// Bytes stored in the repository.
    pub fn size(&self) -> Result<u64> {
        let mut total = 0u64;
        for tpe in rustic_core::ALL_FILE_TYPES {
            total += self
                .be
                .list_with_size(tpe)
                .map_err(classify)?
                .iter()
                .map(|(_, n)| u64::from(*n))
                .sum::<u64>();
        }
        Ok(total)
    }

    /// Changes the passphrase: adds a key for `new` and removes the one
    /// this repository was opened with. Snapshots stay readable.
    pub fn change_passphrase(&mut self, new: &str) -> Result<()> {
        let old = *self.repo()?.key_id();
        self.repo()?
            .add_key(new, &KeyOptions::default())
            .map_err(classify)?;
        // A key can't remove itself: reopen with the new one first.
        let reopened =
            Repository::new_with_progress(&self.options, &self.backends, Bars(self.shared.clone()))
                .and_then(|r| r.open(&Credentials::password(new)))
                .map_err(classify)?;
        if let Some(old) = old {
            reopened.delete_key(&old).map_err(classify)?;
        }
        self.repo = Some(reopened);
        Ok(())
    }

    /// Whether the repository is still usable: an error while indexing can
    /// close it, and the caller then opens a new one.
    pub fn is_open(&self) -> bool {
        self.repo.is_some()
    }
}

/// Maps a rustic error to what the user can act on.
pub(crate) fn classify(e: Box<RusticError>) -> CoreError {
    if e.is_incorrect_password() {
        return BackupError::WrongPassphrase.into();
    }
    let mut src: Option<&(dyn std::error::Error + 'static)> = Some(e.as_ref());
    while let Some(err) = src {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return classify_io(io);
        }
        if let Some(dal) = err.downcast_ref::<opendal::Error>() {
            use opendal::ErrorKind as K;
            return match dal.kind() {
                K::PermissionDenied => BackupError::Credentials,
                K::ConfigInvalid => BackupError::TargetInvalid,
                K::NotFound => BackupError::Unreachable(dal.to_string()),
                _ if dal.is_temporary() => BackupError::Unreachable(dal.to_string()),
                _ => BackupError::Repository(dal.to_string()),
            }
            .into();
        }
        src = err.source();
    }
    BackupError::Repository(short(&e)).into()
}

/// rustic's message line, without its report boilerplate and backtrace.
fn short(e: &RusticError) -> String {
    let text = e.to_string();
    let mut lines = text.lines().skip_while(|l| *l != "Message:").skip(1);
    let msg = lines.next().unwrap_or(&text).trim().to_owned();
    match std::error::Error::source(e) {
        Some(src) => format!("{msg}: {src}"),
        None => msg,
    }
}

fn classify_io(e: &std::io::Error) -> CoreError {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::StorageFull | K::QuotaExceeded => BackupError::Full,
        K::PermissionDenied | K::ReadOnlyFilesystem => BackupError::ReadOnly,
        K::NotFound | K::NotConnected | K::TimedOut | K::ConnectionRefused => {
            BackupError::Unreachable(e.to_string())
        }
        _ => BackupError::Repository(e.to_string()),
    }
    .into()
}
