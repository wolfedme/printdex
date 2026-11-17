//! Bookkeeping for `.printdex.toml` sidecars.

use std::path::PathBuf;

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::grouping::ModelKind;
use crate::ids::{LibraryId, ModelId};

/// Where a model's sidecar goes, if its library writes sidecars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarTarget {
    pub library: LibraryId,
    pub root: PathBuf,
    pub rel_path: String,
    pub kind: ModelKind,
    pub enabled: bool,
    /// The sidecar's mtime when Printdex last wrote or read it.
    pub mtime_ns: Option<i64>,
}

/// A present model with its sidecar bookkeeping, for import on scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidecarState {
    pub id: ModelId,
    pub rel_path: String,
    pub kind: ModelKind,
    pub mtime_ns: Option<i64>,
}

impl Db {
    /// Library and path of model `id`.
    pub async fn sidecar_target(&self, id: ModelId) -> Result<Option<SidecarTarget>> {
        let row = sqlx::query!(
            r#"SELECT l.id as "library!: LibraryId", l.path, m.rel_path, m.kind,
                      l.sidecar_enabled as "enabled: bool", m.sidecar_mtime_ns
               FROM models m JOIN libraries l ON l.id = m.library_id WHERE m.id = ?"#,
            id
        )
        .fetch_optional(self.writer())
        .await?;
        Ok(row.map(|r| SidecarTarget {
            library: r.library,
            root: PathBuf::from(r.path),
            rel_path: r.rel_path,
            kind: ModelKind::from_db(&r.kind).unwrap_or(ModelKind::Folder),
            enabled: r.enabled,
            mtime_ns: r.sidecar_mtime_ns,
        }))
    }

    /// Records the sidecar mtime of model `id` after a write or an import.
    pub async fn set_sidecar_mtime(&self, id: ModelId, mtime_ns: Option<i64>) -> Result<()> {
        sqlx::query!(
            "UPDATE models SET sidecar_mtime_ns = ? WHERE id = ?",
            mtime_ns,
            id
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Present models of `lib` with their sidecar mtimes.
    pub async fn sidecar_states(&self, lib: LibraryId) -> Result<Vec<SidecarState>> {
        let rows = sqlx::query!(
            r#"SELECT id as "id!: ModelId", rel_path, kind, sidecar_mtime_ns FROM models
               WHERE library_id = ? AND missing_since IS NULL"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| SidecarState {
                id: r.id,
                rel_path: r.rel_path,
                kind: ModelKind::from_db(&r.kind).unwrap_or(ModelKind::Folder),
                mtime_ns: r.sidecar_mtime_ns,
            })
            .collect())
    }

    /// Turns sidecar writing on or off for library `lib`.
    pub async fn set_sidecar_enabled(&self, lib: LibraryId, enabled: bool) -> Result<()> {
        let now = crate::time::now_ms();
        let done = sqlx::query!(
            "UPDATE libraries SET sidecar_enabled = ?, updated_at = ? WHERE id = ?",
            enabled,
            now,
            lib
        )
        .execute(self.writer())
        .await?;
        if done.rows_affected() == 0 {
            return Err(CoreError::LibraryNotFound(lib));
        }
        Ok(())
    }

    /// Present models of `lib` that have anything the user wrote.
    pub async fn models_with_data(&self, lib: LibraryId) -> Result<Vec<ModelId>> {
        Ok(sqlx::query_scalar!(
            r#"SELECT id as "id!: ModelId" FROM models m
               WHERE library_id = ? AND missing_since IS NULL
                 AND (data_updated_at IS NOT NULL
                      OR EXISTS (SELECT 1 FROM recipes r WHERE r.model_id = m.id)
                      OR EXISTS (SELECT 1 FROM attempts a WHERE a.model_id = m.id))"#,
            lib
        )
        .fetch_all(self.writer())
        .await?)
    }
}
