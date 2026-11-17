//! SQLite store: one writer connection, a small pool of readers, WAL mode.

mod files;
mod libraries;
mod models;
mod search;

use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

use crate::error::Result;

pub use files::KnownFile;
pub use libraries::{Library, LibraryKind};
pub use models::{AssetInfo, ModelDetail, PartInfo};
pub use search::{ModelHit, SearchFilter, SortOrder, Stats, to_fts_query};

/// Handle to the Printdex database. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Db {
    writer: SqlitePool,
    reader: SqlitePool,
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
        Ok(Db { writer, reader })
    }

    /// The single writer pool. Every write goes through it.
    pub(crate) fn writer(&self) -> &SqlitePool {
        &self.writer
    }

    /// The reader pool, so searches never wait behind a scan's writes.
    pub(crate) fn reader(&self) -> &SqlitePool {
        &self.reader
    }
}
