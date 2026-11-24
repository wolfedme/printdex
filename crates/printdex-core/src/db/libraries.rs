//! Library rows.

use std::path::PathBuf;

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::ids::LibraryId;
use crate::time::now_ms;

/// Where a library lives, detected from its mount (see [`crate::volume`]).
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

    pub fn from_db(s: &str) -> Self {
        match s {
            "removable" => LibraryKind::Removable,
            "network" => LibraryKind::Network,
            _ => LibraryKind::Local,
        }
    }
}

/// How Printdex notices changes in a library.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ChangeMode {
    /// Watch a local folder; check a share or removable drive every
    /// [`ChangeMode::SHARE_MINUTES`] minutes.
    #[default]
    Auto,
    /// Watch the folder for changes.
    Watch,
    /// Rescan every so many minutes.
    Every(u16),
    /// Only when the user asks.
    Manual,
}

impl ChangeMode {
    /// Minutes between checks of a share in [`ChangeMode::Auto`].
    pub const SHARE_MINUTES: u16 = 15;
    /// Intervals offered in Settings, in minutes.
    pub const INTERVALS: [u16; 3] = [5, 15, 60];

    /// What `self` means for a library of `kind`: never `Auto`.
    pub fn resolve(self, kind: LibraryKind) -> ChangeMode {
        match (self, kind) {
            (ChangeMode::Auto, LibraryKind::Local) => ChangeMode::Watch,
            (ChangeMode::Auto, _) => ChangeMode::Every(ChangeMode::SHARE_MINUTES),
            (mode, _) => mode,
        }
    }

    /// Stable text, as stored in the database.
    pub fn to_db(self) -> String {
        match self {
            ChangeMode::Auto => "auto".into(),
            ChangeMode::Watch => "watch".into(),
            ChangeMode::Every(m) => format!("every:{m}"),
            ChangeMode::Manual => "manual".into(),
        }
    }

    /// Parses [`ChangeMode::to_db`]; anything unknown is `Auto`.
    pub fn from_db(s: &str) -> ChangeMode {
        match s {
            "watch" => ChangeMode::Watch,
            "manual" => ChangeMode::Manual,
            _ => s
                .strip_prefix("every:")
                .and_then(|m| m.parse().ok())
                .filter(|&m| m > 0)
                .map_or(ChangeMode::Auto, ChangeMode::Every),
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
    pub change_mode: ChangeMode,
    pub sidecar_enabled: bool,
    pub last_full_scan_at: Option<i64>,
}

struct LibraryRow {
    id: LibraryId,
    path: String,
    display_name: String,
    kind: String,
    change_mode: String,
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
            change_mode: ChangeMode::from_db(&r.change_mode),
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
        let mode = lib.change_mode.to_db();
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO libraries
               (id, path, display_name, kind, change_mode, sidecar_enabled, last_full_scan_at, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            lib.id,
            path,
            lib.display_name,
            kind,
            mode,
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
            r#"SELECT id as "id!: LibraryId", path, display_name, kind, change_mode,
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
            r#"SELECT id as "id!: LibraryId", path, display_name, kind, change_mode,
                      sidecar_enabled as "sidecar_enabled: bool", last_full_scan_at
               FROM libraries WHERE id = ?"#,
            id
        )
        .fetch_optional(self.reader())
        .await?
        .map(Library::from)
        .ok_or(CoreError::LibraryNotFound(id))
    }

    /// Deletes library `id` and, by cascade, its models, parts and assets.
    /// Returns whether a row was deleted. Never touches files.
    pub async fn delete_library(&self, id: LibraryId) -> Result<bool> {
        // Safe because the user confirmed removing the library; its files and sidecars stay on disk.
        let r = sqlx::query!("DELETE FROM libraries WHERE id = ?", id)
            .execute(self.writer())
            .await?;
        Ok(r.rows_affected() > 0)
    }

    /// Sets the display name of library `id`.
    pub async fn rename_library(&self, id: LibraryId, name: &str) -> Result<()> {
        let now = now_ms();
        let r = sqlx::query!(
            "UPDATE libraries SET display_name = ?, updated_at = ? WHERE id = ?",
            name,
            now,
            id
        )
        .execute(self.writer())
        .await?;
        if r.rows_affected() == 0 {
            return Err(CoreError::LibraryNotFound(id));
        }
        Ok(())
    }

    /// Sets how library `id` notices changes.
    pub async fn set_change_mode(&self, id: LibraryId, mode: ChangeMode) -> Result<()> {
        let mode = mode.to_db();
        let now = now_ms();
        let r = sqlx::query!(
            "UPDATE libraries SET change_mode = ?, updated_at = ? WHERE id = ?",
            mode,
            now,
            id
        )
        .execute(self.writer())
        .await?;
        if r.rows_affected() == 0 {
            return Err(CoreError::LibraryNotFound(id));
        }
        Ok(())
    }

    /// Records the detected volume kind of library `id`.
    pub async fn set_library_kind(&self, id: LibraryId, kind: LibraryKind) -> Result<()> {
        let kind = kind.as_str();
        let now = now_ms();
        sqlx::query!(
            "UPDATE libraries SET kind = ?, updated_at = ? WHERE id = ?",
            kind,
            now,
            id
        )
        .execute(self.writer())
        .await?;
        Ok(())
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
            change_mode: crate::db::ChangeMode::Auto,
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
    async fn delete_and_rename_library() {
        let (_d, db) = open().await;
        let a = lib("/a");
        db.insert_library(&a).await.unwrap();
        db.rename_library(a.id, "Workshop").await.unwrap();
        assert_eq!(db.library(a.id).await.unwrap().display_name, "Workshop");
        assert!(db.delete_library(a.id).await.unwrap());
        assert!(!db.delete_library(a.id).await.unwrap());
        assert!(matches!(
            db.rename_library(a.id, "x").await,
            Err(CoreError::LibraryNotFound(_))
        ));
    }

    #[test]
    fn change_mode_round_trips_and_resolves() {
        for m in [
            ChangeMode::Auto,
            ChangeMode::Watch,
            ChangeMode::Every(5),
            ChangeMode::Manual,
        ] {
            assert_eq!(ChangeMode::from_db(&m.to_db()), m);
        }
        assert_eq!(ChangeMode::from_db("every:0"), ChangeMode::Auto);
        assert_eq!(ChangeMode::from_db("???"), ChangeMode::Auto);
        assert_eq!(
            ChangeMode::Auto.resolve(LibraryKind::Local),
            ChangeMode::Watch
        );
        assert_eq!(
            ChangeMode::Auto.resolve(LibraryKind::Network),
            ChangeMode::Every(15)
        );
        assert_eq!(
            ChangeMode::Manual.resolve(LibraryKind::Network),
            ChangeMode::Manual
        );
    }

    #[tokio::test]
    async fn change_mode_is_stored() {
        let (_d, db) = open().await;
        let l = lib("/srv/minis");
        db.insert_library(&l).await.unwrap();
        db.set_change_mode(l.id, ChangeMode::Every(60))
            .await
            .unwrap();
        db.set_library_kind(l.id, LibraryKind::Network)
            .await
            .unwrap();
        let back = db.library(l.id).await.unwrap();
        assert_eq!(back.change_mode, ChangeMode::Every(60));
        assert_eq!(back.kind, LibraryKind::Network);
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
