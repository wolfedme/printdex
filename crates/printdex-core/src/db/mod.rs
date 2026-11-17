//! SQLite store: one writer connection, a small pool of readers, WAL mode.

mod files;
mod libraries;
mod missing;
mod models;
pub(crate) mod numbers;
pub(crate) mod organize;
mod prints;
mod search;
mod sidecars;
mod thumbs;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::Result;

pub use files::KnownFile;
pub use libraries::{Library, LibraryKind};
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
        Ok(Db {
            writer,
            reader,
            photo_dir,
        })
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
