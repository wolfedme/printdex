//! The data snapshot: a consistent copy of the database, the attempt
//! photos and the app's settings files, staged in one folder.
//!
//! Layout: `printdex.db` (from `VACUUM INTO`), `photos/`, and each extra
//! settings file by its name. The backup settings travel inside the
//! database (its `backup_*` tables, never secrets). Caches, logs, locks and
//! credentials are never included.

use std::path::Path;

use crate::db::Db;
use crate::error::{CoreError, Result};

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CoreError + '_ {
    move |source| CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Links (or, across volumes, copies) `photos` and `extras` into `staging`.
pub(crate) fn stage_files(
    staging: &Path,
    photos: &Path,
    extras: &[std::path::PathBuf],
) -> Result<()> {
    if photos.is_dir() {
        link_tree(photos, &staging.join("photos"))?;
    }
    for file in extras {
        if let (true, Some(name)) = (file.is_file(), file.file_name()) {
            link_or_copy(file, &staging.join(name))?;
        }
    }
    Ok(())
}

/// `link_or_copy` for every file under `from` (photos are
/// `<attempt>/<photo>.<ext>`).
fn link_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(io(to))?;
    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let (src, dest) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(io(&src))?;
        if kind.is_dir() {
            link_tree(&src, &dest)?;
        } else if kind.is_file() {
            link_or_copy(&src, &dest)?;
        }
    }
    Ok(())
}

fn link_or_copy(from: &Path, to: &Path) -> Result<()> {
    if std::fs::hard_link(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to).map(|_| ()).map_err(io(from))
}

impl Db {
    /// Writes a consistent copy of the database to `dest` (`VACUUM INTO`,
    /// on a reader connection so writes go on meanwhile).
    pub(crate) async fn copy_into(&self, dest: &Path) -> Result<()> {
        let path = dest.to_string_lossy().into_owned();
        let _ = std::fs::remove_file(dest);
        let on_reader = sqlx::query("VACUUM INTO ?")
            .bind(&path)
            .execute(self.reader())
            .await;
        if let Err(e) = on_reader {
            tracing::debug!(error = %e, "VACUUM INTO on a reader failed; using the writer");
            let _ = std::fs::remove_file(dest);
            sqlx::query("VACUUM INTO ?")
                .bind(&path)
                .execute(self.writer())
                .await?;
        }
        Ok(())
    }
}
