//! Restoring: Printdex's data (staged, swapped in on the next start),
//! whole libraries, and single models.

use std::path::{Path, PathBuf};

use tokio_util::sync::CancellationToken;

use super::repo::{Repo, RepoOptions, SnapshotFilter, SnapshotInfo, Target, TargetSecrets};
use super::run::{DATA_TAG, models_tag};
use crate::db::{Db, Library};
use crate::engine::Engine;
use crate::error::{BackupError, CoreError, Result};
use crate::grouping::ModelKind;
use crate::ids::{LibraryId, ModelId, Uuid};
use crate::scan::ScanSummary;
use crate::time::now_ms;

/// Libraries that ask where they live now, after a data restore.
pub const RELINK: &str = "restore_relink";
/// Libraries whose next full scan rewrites sidecars from the database
/// instead of importing them (the restored point in time wins).
pub const SIDECARS_FROM_DB: &str = "restore_sidecars_from_db";

const STAGING: &str = "restore-staging";
const READY: &str = ".ready";
const DB_FILE: &str = "printdex.db";
const PHOTOS: &str = "photos";

/// What a data restore left for the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredData {
    /// Settings files from the snapshot (`settings.toml`, `apps.toml`) for
    /// the app to put back before it restarts.
    pub settings_files: Vec<PathBuf>,
}

/// Where a single model is restored to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelRestoreDest {
    /// Its own folder: files that differ go to the trash first.
    Original,
    /// Into this folder (a model folder is created inside it).
    Folder(PathBuf),
}

/// How a backed-up file compares with the one on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileChange {
    Same,
    Changed,
    Missing,
}

/// One file of a model in a snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDiff {
    /// Relative to the model folder (the file name for a single file).
    pub rel: String,
    pub change: FileChange,
}

/// A model's place in its library, as backups see it.
struct ModelPlace {
    library: LibraryId,
    root: PathBuf,
    /// Paths inside the library to restore (the folder, or the file and
    /// its sidecar).
    subpaths: Vec<String>,
    /// The folder the subpaths' files go into, relative to the library.
    base: String,
}

fn io_err(path: &Path, source: std::io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn join_err(e: tokio::task::JoinError) -> CoreError {
    BackupError::Repository(e.to_string()).into()
}

/// Moves `from` to `to`, copying when they are on different drives.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
}

fn move_any(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        if std::fs::rename(from, to).is_ok() {
            return Ok(());
        }
        for entry in walk_files(from)? {
            let rel = entry.strip_prefix(from).unwrap_or(&entry);
            move_file(&entry, &to.join(rel))?;
        }
        std::fs::remove_dir_all(from)
    } else {
        move_file(from, to)
    }
}

fn walk_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            out.extend(walk_files(&p)?);
        } else {
            out.push(p);
        }
    }
    Ok(out)
}

fn same_content(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let (ma, mb) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    if ma.len() != mb.len() {
        return Ok(false);
    }
    let (mut fa, mut fb) = (
        std::io::BufReader::new(std::fs::File::open(a)?),
        std::io::BufReader::new(std::fs::File::open(b)?),
    );
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let n = fa.read(&mut ba)?;
        if n == 0 {
            return Ok(true);
        }
        fb.read_exact(&mut bb[..n])?;
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
    }
}

/// Swaps a staged data restore in, before the database is opened. The
/// previous database and photos move to `pre-restore-<time>/`. An
/// unfinished staging folder is removed. Returns whether a restore was
/// applied.
pub(crate) fn apply_staged(data_dir: &Path) -> Result<bool> {
    let staging = data_dir.join(STAGING);
    if !staging.exists() {
        return Ok(false);
    }
    if !staging.join(READY).exists() || !staging.join(DB_FILE).exists() {
        tracing::warn!("removing an unfinished restore");
        std::fs::remove_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
        return Ok(false);
    }
    let old = data_dir.join(format!("pre-restore-{}", now_ms()));
    std::fs::create_dir_all(&old).map_err(|e| io_err(&old, e))?;
    for name in [
        DB_FILE.to_owned(),
        format!("{DB_FILE}-wal"),
        format!("{DB_FILE}-shm"),
        PHOTOS.to_owned(),
    ] {
        let from = data_dir.join(&name);
        if from.exists() {
            move_any(&from, &old.join(&name)).map_err(|e| io_err(&from, e))?;
        }
    }
    for name in [DB_FILE, PHOTOS] {
        let from = staging.join(name);
        if from.exists() {
            let to = data_dir.join(name);
            move_any(&from, &to).map_err(|e| io_err(&from, e))?;
        }
    }
    std::fs::remove_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
    tracing::info!(previous = %old.display(), "applied a restored backup");
    Ok(true)
}

/// Deletes the data a restore replaced (once a new backup exists).
pub(crate) fn remove_pre_restore(data_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(data_dir) else {
        return;
    };
    for e in entries.flatten() {
        if e.file_name().to_string_lossy().starts_with("pre-restore-")
            && let Err(err) = std::fs::remove_dir_all(e.path())
        {
            tracing::warn!(error = %err, "could not remove data a restore replaced");
        }
    }
}

impl Engine {
    async fn open_repo(&self, target: &Target, secrets: &TargetSecrets) -> Result<Repo> {
        let (target, secrets) = (target.clone(), secrets.clone());
        let opts = RepoOptions {
            cache_dir: (!target.is_folder()).then(|| self.cache_dir.join("backup")),
            throttle: None,
        };
        tokio::task::spawn_blocking(move || Repo::open(&target, &secrets, &opts))
            .await
            .map_err(join_err)?
    }

    /// The configured target's repository (for restores inside the app).
    async fn current_repo(&self) -> Result<Repo> {
        let (t, s) = self.current_target().await?;
        self.open_repo(&t.target, &s).await
    }

    /// Data snapshots at `target`, newest first (for "Restore from
    /// backup").
    pub async fn data_snapshots(
        &self,
        target: &Target,
        secrets: &TargetSecrets,
    ) -> Result<Vec<SnapshotInfo>> {
        let repo = self.open_repo(target, secrets).await?;
        let mut snaps = tokio::task::spawn_blocking(move || {
            repo.snapshots(&SnapshotFilter {
                host: None,
                tag: Some(DATA_TAG.into()),
            })
        })
        .await
        .map_err(join_err)??;
        snaps.reverse();
        Ok(snaps)
    }

    /// Restores data snapshot `snapshot` from `target` into a staging
    /// folder; the next [`Engine::open`] swaps it in. The restored install
    /// gets a new id (the old one stays its predecessor), keeps backing up
    /// to `target`, and asks for each library where it lives now. With
    /// `remember`, the secrets are stored for the restored install.
    pub async fn restore_data(
        &self,
        target: &Target,
        secrets: &TargetSecrets,
        snapshot: &str,
        remember: bool,
    ) -> Result<RestoredData> {
        let staging = self.data_dir.join(STAGING);
        if staging.exists() {
            std::fs::remove_dir_all(&staging).map_err(|e| io_err(&staging, e))?;
        }
        let mut repo = self.open_repo(target, secrets).await?;
        let (snap, dest) = (snapshot.to_owned(), staging.clone());
        tokio::task::spawn_blocking(move || {
            repo.restore(&snap, "", &dest, &CancellationToken::new(), None)
        })
        .await
        .map_err(join_err)??;
        let db_path = staging.join(DB_FILE);
        if !db_path.is_file() {
            return Err(BackupError::SnapshotNotFound.into());
        }

        // Prepare the restored database for this computer.
        let db = Db::open(&db_path).await?;
        let prepared = async {
            let old = db.install_id().await?;
            db.add_install_predecessor(&old.hyphenated().to_string())
                .await?;
            let install = Uuid::now_v7();
            db.set_install_id(&install.hyphenated().to_string()).await?;
            let settings = serde_json::to_string(target).map_err(|_| BackupError::TargetInvalid)?;
            let existing = db.backup_targets().await?.into_iter().find(|row| {
                serde_json::from_str::<Target>(&row.settings).is_ok_and(|t| &t == target)
            });
            let kind = super::config::kind(target).to_owned();
            let rows = db
                .set_backup_targets(&[(
                    existing.map_or(0, |r| r.id),
                    kind,
                    settings,
                    !secrets.passphrase.is_empty(),
                )])
                .await?;
            let libs: Vec<LibraryId> = db.libraries().await?.iter().map(|l| l.id).collect();
            db.set_library_list(RELINK, &libs).await?;
            db.set_library_list(SIDECARS_FROM_DB, &libs).await?;
            Ok::<_, CoreError>((install, rows.first().map_or(0, |r| r.id)))
        }
        .await;
        db.close().await;
        let (install, target_id) = prepared?;
        self.set_secrets_for(install, target_id, secrets, remember);

        crate::fsx::durable_write(&staging.join(READY), b"", crate::fsx::Durability::UserData)
            .map_err(|e| io_err(&staging, e))?;
        let settings_files = ["settings.toml", "apps.toml"]
            .iter()
            .map(|n| staging.join(n))
            .filter(|p| p.is_file())
            .collect();
        Ok(RestoredData { settings_files })
    }

    /// Earlier ids of this install (before restores), oldest first.
    pub async fn install_predecessors(&self) -> Result<Vec<String>> {
        self.db.install_predecessors().await
    }

    /// Libraries still waiting for "where does it live now?" after a
    /// restore.
    pub async fn pending_relinks(&self) -> Result<Vec<Library>> {
        let ids = self.db.library_list(RELINK).await?;
        Ok(self
            .db
            .libraries()
            .await?
            .into_iter()
            .filter(|l| ids.contains(&l.id))
            .collect())
    }

    /// "Skip for now": the library stays as it is (offline if its folder
    /// is gone).
    pub async fn skip_relink(&self, id: LibraryId) -> Result<()> {
        self.db.take_from_library_list(RELINK, id).await?;
        Ok(())
    }

    /// Points library `id` at `new_root` and rescans it. Models keep their
    /// data: paths are relative to the root, and content fingerprints
    /// reconnect models that moved.
    pub async fn relink_library(&self, id: LibraryId, new_root: &Path) -> Result<ScanSummary> {
        let path = dunce::canonicalize(new_root)
            .map_err(|_| CoreError::LibraryPathInvalid(new_root.to_path_buf()))?;
        let Some(text) = path.to_str().filter(|_| path.is_dir()) else {
            return Err(CoreError::LibraryPathInvalid(path));
        };
        for other in self.db.libraries().await? {
            if other.id != id && (path.starts_with(&other.path) || other.path.starts_with(&path)) {
                return Err(CoreError::LibraryOverlaps {
                    existing: other.path,
                });
            }
        }
        self.db.set_library_path(id, text).await?;
        self.db.take_from_library_list(RELINK, id).await?;
        if let Ok(lib) = self.db.library(id).await {
            let _ = self
                .events
                .send(crate::events::EngineEvent::LibraryChanged(lib));
        }
        self.rescan(id, CancellationToken::new()).await
    }

    /// Model snapshots of library `id` (from any install), newest first.
    pub async fn library_snapshots(&self, id: LibraryId) -> Result<Vec<SnapshotInfo>> {
        let repo = self.current_repo().await?;
        let tag = models_tag(id);
        let mut snaps = tokio::task::spawn_blocking(move || {
            repo.snapshots(&SnapshotFilter {
                host: None,
                tag: Some(tag),
            })
        })
        .await
        .map_err(join_err)??;
        snaps.reverse();
        Ok(snaps)
    }

    /// "Restore files from backup": restores library `id`'s model files
    /// from `snapshot` into `dest`, then points the library there.
    pub async fn restore_library_files(
        &self,
        id: LibraryId,
        snapshot: &str,
        dest: &Path,
    ) -> Result<ScanSummary> {
        let mut repo = self.current_repo().await?;
        let (snap, out) = (snapshot.to_owned(), dest.to_path_buf());
        tokio::task::spawn_blocking(move || {
            repo.restore(&snap, "", &out, &CancellationToken::new(), None)
        })
        .await
        .map_err(join_err)??;
        self.relink_library(id, dest).await
    }

    async fn model_place(&self, model: ModelId) -> Result<ModelPlace> {
        let t = self
            .db
            .sidecar_target(model)
            .await?
            .ok_or(CoreError::ModelNotFound)?;
        Ok(match t.kind {
            ModelKind::Folder => ModelPlace {
                library: t.library,
                root: t.root,
                subpaths: vec![t.rel_path.clone()],
                base: t.rel_path,
            },
            ModelKind::SingleFile => {
                let base = t
                    .rel_path
                    .rsplit_once('/')
                    .map_or("", |(p, _)| p)
                    .to_owned();
                let sidecar = crate::sidecar::path_for(Path::new(""), &t.rel_path, t.kind)
                    .to_string_lossy()
                    .replace('\\', "/");
                ModelPlace {
                    library: t.library,
                    root: t.root,
                    subpaths: vec![t.rel_path, sidecar],
                    base,
                }
            }
        })
    }

    /// Snapshots that hold model `model`, newest first.
    pub async fn model_snapshots(&self, model: ModelId) -> Result<Vec<SnapshotInfo>> {
        let place = self.model_place(model).await?;
        let mut repo = self.current_repo().await?;
        let tag = models_tag(place.library);
        let sub = place.subpaths[0].clone();
        let found = tokio::task::spawn_blocking(move || {
            repo.find(
                &SnapshotFilter {
                    host: None,
                    tag: Some(tag),
                },
                &sub,
            )
        })
        .await
        .map_err(join_err)??;
        Ok(found
            .into_iter()
            .rev()
            .filter(|(_, files)| !files.is_empty())
            .map(|(s, _)| s)
            .collect())
    }

    /// Restores model `model`'s files from `snapshot` into a fresh
    /// temporary folder. Returns it with the files relative to the model's
    /// base folder.
    async fn stage_model(
        &self,
        place: &ModelPlace,
        snapshot: &str,
    ) -> Result<(PathBuf, Vec<String>)> {
        let tmp = self
            .data_dir
            .join("restore-tmp")
            .join(Uuid::now_v7().to_string());
        let mut repo = self.current_repo().await?;
        let (snap, subs, out) = (snapshot.to_owned(), place.subpaths.clone(), tmp.clone());
        let files = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let mut files = Vec::new();
            for (i, sub) in subs.iter().enumerate() {
                // A folder's contents, or a file, land directly in `out`
                // (the model's base folder). The sidecar of a single file
                // may not exist.
                let r = repo.restore(&snap, sub, &out, &CancellationToken::new(), None);
                match r {
                    Ok(()) => {}
                    Err(_) if i > 0 => continue,
                    Err(e) => return Err(e),
                }
            }
            for f in walk_files(&out).map_err(|e| io_err(&out, e))? {
                files.push(
                    f.strip_prefix(&out)
                        .unwrap_or(&f)
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
            files.sort();
            Ok(files)
        })
        .await
        .map_err(join_err)?;
        match files {
            Ok(f) => Ok((tmp, f)),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&tmp);
                Err(e)
            }
        }
    }

    /// Which of model `model`'s files differ from `snapshot`.
    pub async fn model_restore_preview(
        &self,
        model: ModelId,
        snapshot: &str,
    ) -> Result<Vec<FileDiff>> {
        let place = self.model_place(model).await?;
        let (tmp, files) = self.stage_model(&place, snapshot).await?;
        let here = place.root.join(&place.base);
        let diffs = tokio::task::spawn_blocking(move || {
            let out = files
                .into_iter()
                .map(|rel| {
                    let local = here.join(&rel);
                    let change = if !local.exists() {
                        FileChange::Missing
                    } else if same_content(&tmp.join(&rel), &local).unwrap_or(false) {
                        FileChange::Same
                    } else {
                        FileChange::Changed
                    };
                    FileDiff { rel, change }
                })
                .collect::<Vec<_>>();
            let _ = std::fs::remove_dir_all(&tmp);
            out
        })
        .await
        .map_err(join_err)?;
        Ok(diffs)
    }

    /// Restores model `model` from `snapshot`. Into its own folder, files
    /// that differ go to the trash first and identical ones are left
    /// alone; then the library is rescanned and the model's sidecar from
    /// the snapshot is imported, so its tags, recipe and log return to that
    /// point in time. Returns the files written.
    pub async fn restore_model(
        &self,
        model: ModelId,
        snapshot: &str,
        dest: ModelRestoreDest,
    ) -> Result<Vec<FileDiff>> {
        let place = self.model_place(model).await?;
        let (tmp, files) = self.stage_model(&place, snapshot).await?;
        let into = match &dest {
            ModelRestoreDest::Original => place.root.join(&place.base),
            ModelRestoreDest::Folder(dir) => {
                let name = Path::new(&place.subpaths[0])
                    .file_name()
                    .map(PathBuf::from)
                    .unwrap_or_default();
                if place.subpaths.len() == 1 {
                    dir.join(name)
                } else {
                    dir.clone()
                }
            }
        };
        let (stage, target) = (tmp.clone(), into.clone());
        let written = tokio::task::spawn_blocking(move || -> Result<Vec<FileDiff>> {
            let mut out = Vec::new();
            for rel in files {
                let (from, to) = (stage.join(&rel), target.join(&rel));
                let change = if !to.exists() {
                    FileChange::Missing
                } else if same_content(&from, &to).map_err(|e| io_err(&to, e))? {
                    FileChange::Same
                } else {
                    FileChange::Changed
                };
                match change {
                    FileChange::Same => continue,
                    FileChange::Changed => {
                        trash::delete(&to)
                            .map_err(|e| io_err(&to, std::io::Error::other(e.to_string())))?;
                    }
                    FileChange::Missing => {}
                }
                move_file(&from, &to).map_err(|e| io_err(&to, e))?;
                out.push(FileDiff { rel, change });
            }
            Ok(out)
        })
        .await
        .map_err(join_err);
        let _ = std::fs::remove_dir_all(&tmp);
        let written = written??;
        if dest == ModelRestoreDest::Original {
            self.rescan(place.library, CancellationToken::new()).await?;
            self.import_restored_sidecar(model).await?;
        }
        Ok(written)
    }

    /// Makes model `model`'s data match its sidecar on disk (just restored),
    /// whatever is newer.
    async fn import_restored_sidecar(&self, model: ModelId) -> Result<()> {
        let Some(t) = self.db.sidecar_target(model).await? else {
            return Ok(());
        };
        let path = crate::sidecar::path_for(&t.root, &t.rel_path, t.kind);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(());
        };
        let Ok(mut file) = crate::sidecar::SidecarFile::parse(&text) else {
            return Ok(());
        };
        // Stamped now, so the restored point in time wins over what the
        // database had.
        let now = now_ms();
        file.updated_at = now;
        if let Some(r) = &mut file.recipe {
            r.updated_at = now;
        }
        let predecessors = self.db.install_predecessors().await?;
        crate::sidecar::import(&self.db, model, &file, self.install, &predecessors).await?;
        self.sidecars.queue(model);
        Ok(())
    }

    /// Stores secrets for target `id` of install `install`.
    fn set_secrets_for(&self, install: Uuid, id: i64, secrets: &TargetSecrets, remember: bool) {
        let key = |what: &str| format!("{install}/target-{id}/{what}");
        if let Some(s) = &secrets.secret {
            self.secrets.set(&key("secret"), s, remember);
        }
        self.secrets
            .set(&key("passphrase"), &secrets.passphrase, remember);
    }
}
