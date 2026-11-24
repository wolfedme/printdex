//! SQLite store: one writer connection, a small pool of readers, WAL mode.

pub(crate) mod backup;
pub mod file_ops;
mod files;
mod libraries;
mod missing;
mod models;
pub(crate) mod numbers;
pub(crate) mod organize;
mod prints;
mod search;
mod sidecars;
pub(crate) mod sync;
mod thumbs;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::{CoreError, Result};

pub use files::KnownFile;
pub use libraries::{ChangeMode, Library, LibraryKind};
pub use missing::{MISSING_GRACE_MS, Purged};
pub use models::{AssetInfo, ModelDetail, PartInfo};
pub use numbers::{NumberQuery, format_number, parse_number_query};
pub use organize::Organize;
pub use prints::MAX_PHOTOS;
pub use search::{
    ModelHit, RecipeFacts, Scope, SearchFilter, Smart, SmartCounts, SortOrder, Stats, WEEK_MS,
    to_fts_query,
};
pub use sidecars::{SidecarState, SidecarTarget};
pub use sync::{DeviceRow, EditEntry, FieldRow, SpaceKind, SpaceRow};
pub use thumbs::{IMAGE_EXTENSIONS, ImageSource, ModelSources, PartSource, render_material};

/// Handle to the Printdex database. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Db {
    writer: SqlitePool,
    reader: SqlitePool,
    /// Where attempt photos live: `photos/` next to the database.
    photo_dir: PathBuf,
}

impl Db {
    /// Opens (creating if needed) the database at `path` and runs migrations.
    pub async fn open(path: &Path) -> Result<Db> {
        let base = SqliteConnectOptions::from_str("sqlite://")?
            .filename(path)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));

        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(base.clone().create_if_missing(true))
            .await?;
        sqlx::migrate!("./migrations").run(&writer).await?;

        let reader = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(base.read_only(true))
            .await?;
        let photo_dir = path
            .parent()
            .map_or_else(|| PathBuf::from("photos"), |p| p.join("photos"));
        let db = Db {
            writer,
            reader,
            photo_dir,
        };
        // Parts from before path materials were stored (migration 0009).
        db.fill_path_materials().await?;
        Ok(db)
    }

    /// The single writer pool. Every write goes through it.
    pub(crate) fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    /// The folder attempt photos are stored in.
    pub fn photo_dir(&self) -> &Path {
        &self.photo_dir
    }

    /// The reader pool, so searches never wait behind a scan's writes.
    pub(crate) fn reader(&self) -> &SqlitePool {
        &self.reader
    }
}

/// What a failed [`Db::open`] (or a first query right after it) means for
/// the user, when it is one of the failures they can act on: a database
/// from a newer Printdex, a damaged file, a folder that can't be written,
/// a full disk. Anything else is returned unchanged. `db` is the database
/// file, carried by the new error.
pub(crate) fn classify_open_error(e: CoreError, db: &Path) -> CoreError {
    use sqlx::migrate::MigrateError;
    let kind: Option<fn(PathBuf) -> CoreError> = match &e {
        CoreError::Migrate(MigrateError::VersionMissing(_) | MigrateError::VersionTooNew(..)) => {
            Some(CoreError::DataTooNew)
        }
        CoreError::Migrate(MigrateError::Execute(s) | MigrateError::ExecuteMigration(s, _))
        | CoreError::Db(s) => sqlx_failure(s, db),
        CoreError::Io { source, .. } => io_failure(source),
        _ => None,
    };
    match kind {
        Some(f) => f(db.to_path_buf()),
        None => e,
    }
}

fn sqlx_failure(e: &sqlx::Error, db: &Path) -> Option<fn(PathBuf) -> CoreError> {
    match e {
        sqlx::Error::Io(io) => io_failure(io),
        sqlx::Error::Database(d) => {
            // sqlx reports the extended result code; the low byte is the
            // primary one.
            let code = d.code()?.parse::<i32>().ok()? & 0xff;
            match code {
                SQLITE_CORRUPT | SQLITE_NOTADB => Some(CoreError::DataCorrupt),
                SQLITE_READONLY => Some(CoreError::DataReadOnly),
                SQLITE_CANTOPEN if !writable(db) => Some(CoreError::DataReadOnly),
                SQLITE_FULL => Some(CoreError::DiskFull),
                _ => None,
            }
        }
        _ => None,
    }
}

fn io_failure(e: &std::io::Error) -> Option<fn(PathBuf) -> CoreError> {
    (e.kind() == std::io::ErrorKind::StorageFull).then_some(CoreError::DiskFull)
}

// SQLite primary result codes (https://www.sqlite.org/rescode.html).
const SQLITE_READONLY: i32 = 8;
const SQLITE_CORRUPT: i32 = 11;
const SQLITE_FULL: i32 = 13;
const SQLITE_CANTOPEN: i32 = 14;
const SQLITE_NOTADB: i32 = 26;

/// Whether this process can write the database file (when it exists) and
/// its folder, where SQLite keeps the `-wal` and `-shm` files.
fn writable(db: &Path) -> bool {
    let dir = db.parent().unwrap_or(Path::new("."));
    (!db.exists() || can_write(db)) && can_write(dir)
}

#[cfg(unix)]
fn can_write(path: &Path) -> bool {
    rustix::fs::access(path, rustix::fs::Access::WRITE_OK).is_ok()
}

#[cfg(not(unix))]
fn can_write(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| !m.permissions().readonly())
}

/// [`classify_open_error`], and when that leaves a plain SQLite error,
/// whether `PRAGMA quick_check(1)` finds the file damaged. Runs only after
/// opening failed: the check reads every page, too slow for every start.
pub(crate) async fn diagnose_open_error(e: CoreError, db: &Path) -> CoreError {
    let e = classify_open_error(e, db);
    let plain_sqlite = matches!(
        &e,
        CoreError::Db(sqlx::Error::Database(_))
            | CoreError::Migrate(sqlx::migrate::MigrateError::Execute(sqlx::Error::Database(
                _
            )))
            | CoreError::Migrate(sqlx::migrate::MigrateError::ExecuteMigration(
                sqlx::Error::Database(_),
                _
            ))
    );
    if plain_sqlite && quick_check_fails(db).await {
        return CoreError::DataCorrupt(db.to_path_buf());
    }
    e
}

/// Runs `PRAGMA quick_check(1)` on a read-only connection: true when it
/// reports a problem or can't run because the file is damaged.
async fn quick_check_fails(db: &Path) -> bool {
    use sqlx::{ConnectOptions, Connection};
    let Ok(opts) = SqliteConnectOptions::from_str("sqlite://") else {
        return false;
    };
    let check = async {
        let mut conn = opts.filename(db).read_only(true).connect().await?;
        let row: (String,) = sqlx::query_as("PRAGMA quick_check(1)")
            .fetch_one(&mut conn)
            .await?;
        let _ = conn.close().await;
        Ok::<_, sqlx::Error>(row.0)
    };
    match check.await {
        Ok(result) => result != "ok",
        Err(e) => matches!(
            classify_open_error(CoreError::Db(e), db),
            CoreError::DataCorrupt(_)
        ),
    }
}

#[cfg(test)]
mod open_error_tests {
    use super::*;

    fn db() -> PathBuf {
        PathBuf::from("/data/printdex.db")
    }

    #[test]
    fn classify_maps_disk_full() {
        let e = CoreError::Io {
            path: PathBuf::from("/data"),
            source: std::io::Error::from(std::io::ErrorKind::StorageFull),
        };
        assert!(matches!(classify_open_error(e, &db()), CoreError::DiskFull(p) if p == db()));
        let e = CoreError::Db(sqlx::Error::Io(std::io::Error::from(
            std::io::ErrorKind::StorageFull,
        )));
        assert!(matches!(
            classify_open_error(e, &db()),
            CoreError::DiskFull(_)
        ));
    }

    #[test]
    fn classify_maps_newer_migrations() {
        let e = CoreError::Migrate(sqlx::migrate::MigrateError::VersionMissing(9999));
        assert!(matches!(classify_open_error(e, &db()), CoreError::DataTooNew(p) if p == db()));
    }

    #[test]
    fn classify_leaves_other_errors() {
        let e = classify_open_error(CoreError::Cancelled, &db());
        assert!(matches!(e, CoreError::Cancelled));
        let e = CoreError::Io {
            path: db(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        };
        assert!(matches!(
            classify_open_error(e, &db()),
            CoreError::Io { .. }
        ));
    }
}

#[cfg(test)]
mod migration_tests {
    /// Two branches merged migrations with the same number once (0008),
    /// which made every fresh database fail to open.
    #[test]
    fn migration_versions_are_unique() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
        let mut seen = std::collections::HashMap::new();
        for e in std::fs::read_dir(&dir).expect("migrations folder") {
            let name = e.expect("entry").file_name().to_string_lossy().into_owned();
            let version = name.split('_').next().unwrap_or_default().to_owned();
            if let Some(other) = seen.insert(version.clone(), name.clone()) {
                panic!("migrations {other} and {name} share version {version}");
            }
        }
    }
}
