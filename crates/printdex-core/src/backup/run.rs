//! Running backup jobs: one at a time, on a blocking thread, with events,
//! a history row and a cached open repository.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use tokio_util::sync::CancellationToken;

use super::config::TargetConfig;
use super::repo::{
    BackupJob, Globs, ProgressFn, Repo, RepoOptions, SnapshotInfo, Target, TargetSecrets,
};
use super::{BackupKind, data};
use crate::engine::Engine;
use crate::error::{BackupError, CoreError, Result};
use crate::events::EngineEvent;
use crate::ids::LibraryId;

/// Snapshot tag of Printdex's own data.
pub const DATA_TAG: &str = "printdex-data";

/// Snapshot tag of one library's model files.
pub fn models_tag(library: LibraryId) -> String {
    format!("printdex-models:{library}")
}

/// What [`Engine::backup_now`] backs up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupScope {
    Data,
    Models(LibraryId),
    /// Data, then every library with model backup on.
    All,
}

/// The result of one backup job.
#[derive(Clone, Debug, PartialEq)]
pub struct RunSummary {
    pub kind: BackupKind,
    /// The new snapshot; `None` when the job was skipped (library offline).
    pub snapshot: Option<SnapshotInfo>,
}

/// Thins this install's snapshots of `tag` to the retention policy
/// ([`super::KEEP`]); other installs' snapshots are never touched. A
/// failure only logs: the snapshot just made is safe either way.
fn thin(repo: &mut Repo, host: &str, tag: &str) {
    match repo.forget(super::KEEP, host, tag) {
        Ok(0) => {}
        Ok(n) => tracing::info!(n, tag, "thinned backup snapshots"),
        Err(e) => tracing::warn!(error = %e, tag, "could not thin backup snapshots"),
    }
}

fn row(kind: BackupKind) -> (&'static str, Option<LibraryId>) {
    match kind {
        BackupKind::Data => ("data", None),
        BackupKind::Models(l) => ("models", Some(l)),
        BackupKind::Prune => ("prune", None),
        BackupKind::Check => ("check", None),
    }
}

/// The repository kept open between jobs, with what it was opened with.
struct Cached {
    target: Target,
    secrets: TargetSecrets,
    repo: Repo,
}

/// Backup state shared by engine clones.
#[derive(Default)]
pub struct BackupState {
    /// Held for the length of a job, so jobs never overlap.
    job: tokio::sync::Mutex<Option<Cached>>,
    cancel: Mutex<CancellationToken>,
    /// Settings files the app wants in the data snapshot.
    extra_files: Mutex<Vec<PathBuf>>,
    /// What the scheduler last found needs attention.
    pub(crate) attention: Mutex<Vec<super::Attention>>,
}

impl std::fmt::Debug for BackupState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BackupState")
    }
}

impl Engine {
    /// Files the data snapshot includes besides the database and photos
    /// (the app's settings.toml and apps.toml). Missing files are skipped.
    pub fn set_backup_files(&self, files: Vec<PathBuf>) {
        *self
            .backup
            .extra_files
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = files;
    }

    /// Stops the running backup job, if any. It fails with
    /// [`CoreError::Cancelled`] and leaves the repository valid.
    pub fn cancel_backup(&self) {
        self.backup
            .cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancel();
    }

    /// The newest `limit` backup jobs, newest first.
    pub async fn backup_runs(&self, limit: u32) -> Result<Vec<crate::db::backup::RunRow>> {
        self.db.backup_runs(i64::from(limit)).await
    }

    /// Backs up now.
    pub async fn backup_now(&self, scope: BackupScope) -> Result<Vec<RunSummary>> {
        match scope {
            BackupScope::Data => Ok(vec![self.backup_data().await?]),
            BackupScope::Models(lib) => Ok(vec![self.backup_models(lib).await?]),
            BackupScope::All => {
                let mut out = vec![self.backup_data().await?];
                let config = self.backup_config().await?;
                for (lib, on) in config.libraries {
                    if on {
                        out.push(self.backup_models(lib).await?);
                    }
                }
                Ok(out)
            }
        }
    }

    /// The target to back up to and its secrets.
    pub(crate) async fn current_target(&self) -> Result<(TargetConfig, TargetSecrets)> {
        let config = self.backup_config().await?;
        let target = config
            .targets
            .into_iter()
            .next()
            .ok_or(BackupError::NotConfigured)?;
        let secrets = self
            .target_secrets(&target)
            .ok_or(BackupError::SecretsNeeded)?;
        Ok((target, secrets))
    }

    /// Runs `f` on the open repository (opening or creating it first) on a
    /// blocking thread, with events and a history row for `kind`. `f`
    /// returns the snapshot it made, if any.
    pub(crate) async fn run_job<F>(&self, kind: BackupKind, f: F) -> Result<RunSummary>
    where
        F: FnOnce(&mut Repo, &CancellationToken, ProgressFn) -> Result<Option<SnapshotInfo>>
            + Send
            + 'static,
    {
        let (row_kind, library) = row(kind);
        let (target, secrets) = match self.current_target().await {
            Ok(t) => t,
            Err(e) => return Err(self.job_failed(kind, None, e).await),
        };
        let mut slot = self.backup.job.lock().await;
        let run = self.db.begin_backup_run(row_kind, library).await?;
        let _ = self.events.send(EngineEvent::BackupStarted { kind });
        let cancel = CancellationToken::new();
        *self
            .backup
            .cancel
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = cancel.clone();
        let events = self.events.clone();
        let progress: ProgressFn = Arc::new(move |done, total| {
            let _ = events.send(EngineEvent::BackupProgress { kind, done, total });
        });
        let cached = slot
            .take()
            .filter(|c| c.target == target.target && c.secrets == secrets);
        let opts = RepoOptions {
            cache_dir: (!target.target.is_folder()).then(|| self.cache_dir.join("backup")),
            throttle: self
                .backup_config()
                .await?
                .schedule
                .upload_limit_kib
                .map(|k| k.saturating_mul(1024)),
        };
        let joined = tokio::task::spawn_blocking(move || {
            let mut repo = match cached {
                Some(c) => c.repo,
                None => match Repo::open_or_init(&target.target, &secrets, &opts) {
                    Ok(r) => r,
                    Err(e) => return (None, Err(e)),
                },
            };
            let out = f(&mut repo, &cancel, progress);
            let keep = repo.is_open().then_some(Cached {
                target: target.target,
                secrets,
                repo,
            });
            (keep, out)
        })
        .await;
        let (keep, out) = match joined {
            Ok(v) => v,
            Err(e) => (None, Err(BackupError::Repository(e.to_string()).into())),
        };
        *slot = keep;
        drop(slot);
        match out {
            Ok(snapshot) => {
                let added = snapshot
                    .as_ref()
                    .map_or(0, |s| i64::try_from(s.bytes_added).unwrap_or(i64::MAX));
                self.db.finish_backup_run(run, "ok", added, None).await?;
                let _ = self.events.send(EngineEvent::BackupFinished {
                    kind,
                    snapshot: snapshot.clone(),
                });
                Ok(RunSummary { kind, snapshot })
            }
            Err(e) => Err(self.job_failed(kind, Some(run), e).await),
        }
    }

    /// Records a job that was skipped (its library is offline).
    pub(crate) async fn job_skipped(&self, kind: BackupKind) -> Result<RunSummary> {
        let (row_kind, library) = row(kind);
        let run = self.db.begin_backup_run(row_kind, library).await?;
        self.db
            .finish_backup_run(run, "skipped", 0, Some("offline"))
            .await?;
        let _ = self.events.send(EngineEvent::BackupFinished {
            kind,
            snapshot: None,
        });
        Ok(RunSummary {
            kind,
            snapshot: None,
        })
    }

    /// Records a failed job and tells subscribers. Returns the error.
    async fn job_failed(&self, kind: BackupKind, run: Option<i64>, e: CoreError) -> CoreError {
        let result = if matches!(e, CoreError::Cancelled) {
            "cancelled"
        } else {
            "failed"
        };
        if let Some(run) = run {
            let msg = e.to_string();
            if let Err(db) = self.db.finish_backup_run(run, result, 0, Some(&msg)).await {
                tracing::warn!(error = %db, "could not record the backup run");
            }
        }
        let e = Arc::new(e);
        let _ = self.events.send(EngineEvent::BackupFailed {
            kind,
            error: e.clone(),
        });
        Arc::try_unwrap(e).unwrap_or_else(|e| crate::scan::clone_error(&e))
    }

    /// Backs up Printdex's own data.
    async fn backup_data(&self) -> Result<RunSummary> {
        let staging = self.data_dir.join("backup-staging");
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging).map_err(|source| CoreError::Io {
            path: staging.clone(),
            source,
        })?;
        let prepared = self.stage_data(&staging).await;
        let out = match prepared {
            Ok(()) => {
                let (src, host) = (staging.clone(), self.install.to_string());
                self.run_job(BackupKind::Data, move |repo, cancel, progress| {
                    let snap = repo.backup(
                        &BackupJob {
                            source: &src,
                            as_path: "/printdex-data",
                            filter: &Globs::default(),
                            tags: &[DATA_TAG.to_owned()],
                            host: &host,
                        },
                        cancel,
                        Some(progress),
                    )?;
                    thin(repo, &host, DATA_TAG);
                    Ok(Some(snap))
                })
                .await
            }
            Err(e) => Err(e),
        };
        let _ = std::fs::remove_dir_all(&staging);
        if out.is_ok() {
            // A new backup exists, so the data a restore replaced can go.
            super::restore::remove_pre_restore(&self.data_dir);
        }
        out
    }

    /// Fills `staging` with the data snapshot's files.
    async fn stage_data(&self, staging: &Path) -> Result<()> {
        self.db.copy_into(&staging.join("printdex.db")).await?;
        let photos = self.db.photo_dir().to_path_buf();
        let extras = self
            .backup
            .extra_files
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let staging = staging.to_path_buf();
        tokio::task::spawn_blocking(move || data::stage_files(&staging, &photos, &extras))
            .await
            .map_err(|e| BackupError::Repository(e.to_string()))?
    }

    /// Backs up one library's model files (skipped while it is offline),
    /// tagged `printdex-models:<library>`, with the scanner's rules.
    async fn backup_models(&self, library: LibraryId) -> Result<RunSummary> {
        let lib = self.db.library(library).await?;
        let kind = BackupKind::Models(library);
        if crate::engine::folder_status(self.fs.clone(), lib.path.clone()).await
            == crate::engine::LibraryStatus::Offline
        {
            return self.job_skipped(kind).await;
        }
        let (root, host, tag) = (
            lib.path.clone(),
            self.install.to_string(),
            models_tag(library),
        );
        let as_path = format!("/models/{library}");
        self.run_job(kind, move |repo, cancel, progress| {
            let snap = repo.backup(
                &BackupJob {
                    source: &root,
                    as_path: &as_path,
                    filter: &crate::walk::Rules::backup_globs(),
                    tags: std::slice::from_ref(&tag),
                    host: &host,
                },
                cancel,
                Some(progress),
            )?;
            thin(repo, &host, &tag);
            Ok(Some(snap))
        })
        .await
    }

    /// The repository's size on the target, or `None` when nothing has
    /// been backed up there yet. Waits for a running job.
    pub async fn backup_size(&self) -> Result<Option<u64>> {
        let (target, secrets) = self.current_target().await?;
        let mut slot = self.backup.job.lock().await;
        let cached = slot
            .take()
            .filter(|c| c.target == target.target && c.secrets == secrets);
        let opts = RepoOptions {
            cache_dir: (!target.target.is_folder()).then(|| self.cache_dir.join("backup")),
            throttle: None,
        };
        let joined = tokio::task::spawn_blocking(move || {
            let repo = match cached {
                Some(c) => c.repo,
                None => match Repo::open(&target.target, &secrets, &opts) {
                    Ok(r) => r,
                    Err(crate::CoreError::Backup(BackupError::NoRepository)) => {
                        return (None, Ok(None));
                    }
                    Err(e) => return (None, Err(e)),
                },
            };
            let out = repo.size().map(Some);
            let keep = repo.is_open().then_some(Cached {
                target: target.target,
                secrets,
                repo,
            });
            (keep, out)
        })
        .await;
        let (keep, out) =
            joined.unwrap_or_else(|e| (None, Err(BackupError::Repository(e.to_string()).into())));
        *slot = keep;
        out
    }

    /// About how many bytes a first model backup of `library` reads (the
    /// size of its files; dedup and compression make the upload smaller).
    pub async fn estimate_models(&self, library: LibraryId) -> Result<u64> {
        self.db.library_bytes(library).await
    }
}
