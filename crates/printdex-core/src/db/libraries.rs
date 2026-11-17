//! Library rows.

use std::path::PathBuf;

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::ids::LibraryId;
use crate::time::now_ms;

/// Where a library lives. Always `Local` until detection lands (milestone 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryKind {
    Local,
    Removable,
    Network,
}

impl LibraryKind {
    /// Stable name, as stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            LibraryKind::Local => "local",
            LibraryKind::Removable => "removable",
            LibraryKind::Network => "network",
        }
    }

    fn from_db(s: &str) -> Self {
        match s {
            "removable" => LibraryKind::Removable,
            "network" => LibraryKind::Network,
            _ => LibraryKind::Local,
        }
    }
}

/// A registered library folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Library {
    pub id: LibraryId,
    pub path: PathBuf,
    pub display_name: String,
    pub kind: LibraryKind,
    pub sidecar_enabled: bool,
    pub last_full_scan_at: Option<i64>,
}

struct LibraryRow {
    id: LibraryId,
    path: String,
    display_name: String,
    kind: String,
    sidecar_enabled: bool,
    last_full_scan_at: Option<i64>,
}

impl From<LibraryRow> for Library {
    fn from(r: LibraryRow) -> Self {
        Library {
            id: r.id,
            path: PathBuf::from(r.path),
            display_name: r.display_name,
            kind: LibraryKind::from_db(&r.kind),
            sidecar_enabled: r.sidecar_enabled,
            last_full_scan_at: r.last_full_scan_at,
        }
    }
}

impl Db {
    /// Inserts a new library row.
    pub async fn insert_library(&self, lib: &Library) -> Result<()> {
        let path = lib
            .path
            .to_str()
            .ok_or_else(|| CoreError::LibraryPathInvalid(lib.path.clone()))?;
        let kind = lib.kind.as_str();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO libraries
               (id, path, display_name, kind, sidecar_enabled, last_full_scan_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            lib.id,
            path,
            lib.display_name,
            kind,
            lib.sidecar_enabled,
            lib.last_full_scan_at,
            now,
            now
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// All libraries, oldest first.
    pub async fn libraries(&self) -> Result<Vec<Library>> {
        let rows = sqlx::query_as!(
            LibraryRow,
            r#"SELECT id as "id!: LibraryId", path, display_name, kind,
                      sidecar_enabled as "sidecar_enabled: bool", last_full_scan_at
               FROM libraries ORDER BY id"#
        )
        .fetch_all(self.reader())
        .await?;
        Ok(rows.into_iter().map(Library::from).collect())
    }

    /// One library, or `LibraryNotFound`.
    pub async fn library(&self, id: LibraryId) -> Result<Library> {
        sqlx::query_as!(
            LibraryRow,
            r#"SELECT id as "id!: LibraryId", path, display_name, kind,
                      sidecar_enabled as "sidecar_enabled: bool", last_full_scan_at
               FROM libraries WHERE id = ?"#,
            id
        )
        .fetch_optional(self.reader())
        .await?
        .map(Library::from)
        .ok_or(CoreError::LibraryNotFound(id))
    }

    /// Records when a full scan of `id` finished.
    pub async fn set_last_full_scan(&self, id: LibraryId, at_ms: i64) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "UPDATE libraries SET last_full_scan_at = ?, updated_at = ? WHERE id = ?",
            at_ms,
            now,
            id
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open() -> (tempfile::TempDir, Db) {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open(&d.path().join("t.db")).await.unwrap();
        (d, db)
    }

    fn lib(path: &str) -> Library {
        Library {
            id: LibraryId::new(),
            path: PathBuf::from(path),
            display_name: "Minis".into(),
            kind: LibraryKind::Local,
            sidecar_enabled: true,
            last_full_scan_at: Some(42),
        }
    }

    #[tokio::test]
    async fn open_runs_migrations_and_enables_wal() {
        let (_d, db) = open().await;
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(db.writer())
            .await
            .unwrap();
        assert_eq!(mode, "wal");
        let fk: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
            .fetch_one(db.writer())
            .await
            .unwrap();
        assert_eq!(fk, 1);
    }

    #[tokio::test]
    async fn open_is_idempotent() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("t.db");
        drop(Db::open(&p).await.unwrap());
        Db::open(&p).await.unwrap();
    }

    #[tokio::test]
    async fn insert_and_list_library() {
        let (_d, db) = open().await;
        let l = lib("/srv/minis");
        db.insert_library(&l).await.unwrap();
        assert_eq!(db.libraries().await.unwrap(), vec![l.clone()]);
        assert_eq!(db.library(l.id).await.unwrap(), l);
        db.set_last_full_scan(l.id, 99).await.unwrap();
        assert_eq!(db.library(l.id).await.unwrap().last_full_scan_at, Some(99));
    }

    #[tokio::test]
    async fn unknown_library_is_not_found() {
        let (_d, db) = open().await;
        assert!(matches!(
            db.library(LibraryId::new()).await,
            Err(CoreError::LibraryNotFound(_))
        ));
    }

    #[tokio::test]
    async fn fts_trigger_indexes_models() {
        let (_d, db) = open().await;
        let l = lib("/srv/minis");
        db.insert_library(&l).await.unwrap();
        sqlx::query(
            "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
             VALUES (?, ?, 'Red Dragon', 'Red Dragon', 'folder', 1, 1)",
        )
        .bind(crate::ids::ModelId::new())
        .bind(l.id)
        .execute(db.writer())
        .await
        .unwrap();
        let n: i64 =
            sqlx::query_scalar("SELECT count(*) FROM models_fts WHERE models_fts MATCH 'dragon'")
                .fetch_one(db.reader())
                .await
                .unwrap();
        assert_eq!(n, 1);
    }
}
