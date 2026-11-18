//! Backup configuration and run history.

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::ids::LibraryId;
use crate::time::now_ms;

/// A stored target: its id, kind, settings JSON (no secrets) and whether
/// it is encrypted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetRow {
    pub id: i64,
    pub settings: String,
    pub encrypted: bool,
}

/// One backup job in the history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunRow {
    pub id: i64,
    pub kind: String,
    pub library_id: Option<LibraryId>,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub result: String,
    pub bytes_added: i64,
    pub error: Option<String>,
}

fn json_err(e: serde_json::Error) -> CoreError {
    CoreError::Db(sqlx::Error::Decode(Box::new(e)))
}

impl Db {
    pub async fn backup_targets(&self) -> Result<Vec<TargetRow>> {
        Ok(sqlx::query_as!(
            TargetRow,
            r#"SELECT id as "id!", settings, encrypted as "encrypted: bool"
               FROM backup_targets ORDER BY id"#
        )
        .fetch_all(self.writer())
        .await?)
    }

    /// Replaces the targets: rows with an id that still exists are updated,
    /// `id == 0` rows are added, the rest removed. Returns the new rows.
    pub async fn set_backup_targets(
        &self,
        targets: &[(i64, String, String, bool)],
    ) -> Result<Vec<TargetRow>> {
        let mut tx = self.writer().begin().await?;
        let keep: Vec<i64> = targets.iter().map(|t| t.0).filter(|id| *id != 0).collect();
        let existing = sqlx::query_scalar!(r#"SELECT id as "id!" FROM backup_targets"#)
            .fetch_all(&mut *tx)
            .await?;
        for id in existing.iter().filter(|id| !keep.contains(id)) {
            sqlx::query!("DELETE FROM backup_targets WHERE id = ?", id)
                .execute(&mut *tx)
                .await?;
        }
        let now = now_ms();
        for (id, kind, settings, encrypted) in targets {
            if *id != 0 && existing.contains(id) {
                sqlx::query!(
                    "UPDATE backup_targets SET kind = ?, settings = ?, encrypted = ? WHERE id = ?",
                    kind,
                    settings,
                    encrypted,
                    id
                )
                .execute(&mut *tx)
                .await?;
            } else {
                sqlx::query!(
                    "INSERT INTO backup_targets (kind, settings, encrypted, created_at)
                     VALUES (?, ?, ?, ?)",
                    kind,
                    settings,
                    encrypted,
                    now
                )
                .execute(&mut *tx)
                .await?;
            }
        }
        tx.commit().await?;
        self.backup_targets().await
    }

    pub async fn set_target_encrypted(&self, id: i64, encrypted: bool) -> Result<()> {
        sqlx::query!(
            "UPDATE backup_targets SET encrypted = ? WHERE id = ?",
            encrypted,
            id
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Libraries with a model-backup choice: (library, on).
    pub async fn backup_libraries(&self) -> Result<Vec<(LibraryId, bool)>> {
        let rows = sqlx::query!(
            r#"SELECT library_id as "library_id!: LibraryId", models_on as "models_on: bool"
               FROM backup_libraries ORDER BY library_id"#
        )
        .fetch_all(self.writer())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.library_id, r.models_on))
            .collect())
    }

    /// Sets the model-backup choices. Libraries not listed keep theirs.
    pub async fn set_backup_libraries(&self, libs: &[(LibraryId, bool)]) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        for (id, on) in libs {
            sqlx::query!(
                "INSERT INTO backup_libraries (library_id, models_on) VALUES (?, ?)
                 ON CONFLICT (library_id) DO UPDATE SET models_on = excluded.models_on",
                id,
                on
            )
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A JSON value from `meta`.
    pub async fn meta_json<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let v = sqlx::query_scalar!("SELECT value FROM meta WHERE key = ?", key)
            .fetch_optional(self.writer())
            .await?;
        v.map(|v| serde_json::from_str(&v).map_err(json_err))
            .transpose()
    }

    pub async fn set_meta_json<T: serde::Serialize>(&self, key: &str, value: &T) -> Result<()> {
        let v = serde_json::to_string(value).map_err(json_err)?;
        sqlx::query!(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            key,
            v
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Starts a history row (result `running`).
    pub async fn begin_backup_run(&self, kind: &str, library: Option<LibraryId>) -> Result<i64> {
        let now = now_ms();
        let r = sqlx::query!(
            "INSERT INTO backup_runs (kind, library_id, started_at, result) VALUES (?, ?, ?, 'running')",
            kind,
            library,
            now
        )
        .execute(self.writer())
        .await?;
        Ok(r.last_insert_rowid())
    }

    pub async fn finish_backup_run(
        &self,
        id: i64,
        result: &str,
        bytes_added: i64,
        error: Option<&str>,
    ) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "UPDATE backup_runs SET finished_at = ?, result = ?, bytes_added = ?, error = ? WHERE id = ?",
            now,
            result,
            bytes_added,
            error,
            id
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// The newest `limit` runs, newest first.
    pub async fn backup_runs(&self, limit: i64) -> Result<Vec<RunRow>> {
        Ok(sqlx::query_as!(
            RunRow,
            r#"SELECT id as "id!", kind, library_id as "library_id: LibraryId", started_at,
                      finished_at, result, bytes_added, error
               FROM backup_runs ORDER BY id DESC LIMIT ?"#,
            limit
        )
        .fetch_all(self.writer())
        .await?)
    }

    /// When a job of `kind` (for `library`) last finished well.
    pub async fn last_ok_run(&self, kind: &str, library: Option<LibraryId>) -> Result<Option<i64>> {
        Ok(sqlx::query_scalar!(
            r#"SELECT MAX(finished_at) as "at: i64" FROM backup_runs
               WHERE kind = ? AND result = 'ok' AND library_id IS ?"#,
            kind,
            library
        )
        .fetch_one(self.writer())
        .await?)
    }

    /// Runs a crash left `running` count as failed.
    pub async fn close_stale_runs(&self) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "UPDATE backup_runs SET result = 'failed', finished_at = ?, error = 'interrupted'
             WHERE result = 'running'",
            now
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Bytes of the files a library holds (parts and other files of
    /// present models).
    pub async fn library_bytes(&self, library: LibraryId) -> Result<u64> {
        let n = sqlx::query_scalar!(
            r#"SELECT
                 (SELECT COALESCE(SUM(size), 0) FROM parts WHERE library_id = ?1 AND missing_since IS NULL)
               + (SELECT COALESCE(SUM(size), 0) FROM assets WHERE library_id = ?1 AND missing_since IS NULL)
               as "n!: i64""#,
            library
        )
        .fetch_one(self.reader())
        .await?;
        Ok(u64::try_from(n).unwrap_or(0))
    }

    pub async fn install_predecessors(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar!(
            r#"SELECT install_id as "install_id!" FROM install_predecessors ORDER BY added_at"#
        )
        .fetch_all(self.writer())
        .await?)
    }

    pub async fn add_install_predecessor(&self, id: &str) -> Result<()> {
        let now = now_ms();
        sqlx::query!(
            "INSERT OR IGNORE INTO install_predecessors (install_id, added_at) VALUES (?, ?)",
            id,
            now
        )
        .execute(self.writer())
        .await?;
        Ok(())
    }
}
