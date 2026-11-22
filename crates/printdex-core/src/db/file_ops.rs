//! Rows for file actions: the journal, and moving a model's rows to where
//! its files went.

use sqlx::Row;

use crate::db::Db;
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::ids::{LibraryId, ModelId};
use crate::time::now_ms;

/// What a file action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Rename,
    Move,
    Trash,
}

impl OpKind {
    fn as_str(self) -> &'static str {
        match self {
            OpKind::Rename => "rename",
            OpKind::Move => "move",
            OpKind::Trash => "trash",
        }
    }

    fn from_db(s: &str) -> OpKind {
        match s {
            "rename" => OpKind::Rename,
            "trash" => OpKind::Trash,
            _ => OpKind::Move,
        }
    }
}

/// Where a file action is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpState {
    Planned,
    Done,
    Undone,
    Failed,
}

impl OpState {
    fn as_str(self) -> &'static str {
        match self {
            OpState::Planned => "planned",
            OpState::Done => "done",
            OpState::Undone => "undone",
            OpState::Failed => "failed",
        }
    }

    fn from_db(s: &str) -> OpState {
        match s {
            "planned" => OpState::Planned,
            "done" => OpState::Done,
            "undone" => OpState::Undone,
            _ => OpState::Failed,
        }
    }
}

/// How one model's files were handled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum How {
    /// Renamed in place (same filesystem).
    Rename,
    /// Copied, checked, then the original moved to the trash (another drive).
    Copy,
    /// Moved to the trash.
    Trash,
}

impl How {
    fn as_str(self) -> &'static str {
        match self {
            How::Rename => "rename",
            How::Copy => "copy",
            How::Trash => "trash",
        }
    }

    fn from_db(s: &str) -> How {
        match s {
            "copy" => How::Copy,
            "trash" => How::Trash,
            _ => How::Rename,
        }
    }
}

/// One model in a file action.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub model: ModelId,
    pub kind: ModelKind,
    pub name: String,
    pub new_name: String,
    pub from_lib: LibraryId,
    pub from_rel: String,
    pub to_lib: LibraryId,
    pub to_rel: String,
    pub how: How,
}

/// A journal entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpRow {
    pub id: i64,
    pub kind: OpKind,
    pub state: OpState,
    pub at: i64,
    pub steps: Vec<Step>,
}

impl Db {
    /// Writes a planned file action with its steps; returns its id.
    pub async fn insert_op(&self, kind: OpKind, steps: &[Step]) -> Result<i64> {
        let mut tx = self.writer().begin().await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO file_ops (kind, state, at) VALUES (?, 'planned', ?) RETURNING id",
        )
        .bind(kind.as_str())
        .bind(now_ms())
        .fetch_one(&mut *tx)
        .await?;
        for (seq, s) in steps.iter().enumerate() {
            sqlx::query(
                "INSERT INTO file_op_steps
                   (op_id, seq, model_id, model_kind, name, new_name, from_lib, from_rel, to_lib, to_rel, how)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(seq as i64)
            .bind(s.model)
            .bind(s.kind.as_str())
            .bind(&s.name)
            .bind(&s.new_name)
            .bind(s.from_lib)
            .bind(&s.from_rel)
            .bind(s.to_lib)
            .bind(&s.to_rel)
            .bind(s.how.as_str())
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Records how step `seq` of op `id` was done (a move that had to copy).
    pub async fn set_step_how(&self, id: i64, seq: usize, how: How) -> Result<()> {
        sqlx::query("UPDATE file_op_steps SET how = ? WHERE op_id = ? AND seq = ?")
            .bind(how.as_str())
            .bind(id)
            .bind(seq as i64)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    pub async fn set_op_state(&self, id: i64, state: OpState) -> Result<()> {
        sqlx::query("UPDATE file_ops SET state = ? WHERE id = ?")
            .bind(state.as_str())
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    async fn op_steps(&self, id: i64) -> Result<Vec<Step>> {
        let rows = sqlx::query(
            "SELECT model_id, model_kind, name, new_name, from_lib, from_rel, to_lib, to_rel, how
             FROM file_op_steps WHERE op_id = ? ORDER BY seq",
        )
        .bind(id)
        .fetch_all(self.reader())
        .await?;
        rows.iter()
            .map(|r| {
                Ok(Step {
                    model: r.try_get("model_id")?,
                    kind: ModelKind::from_db(r.try_get("model_kind")?).unwrap_or(ModelKind::Folder),
                    name: r.try_get("name")?,
                    new_name: r.try_get("new_name")?,
                    from_lib: r.try_get("from_lib")?,
                    from_rel: r.try_get("from_rel")?,
                    to_lib: r.try_get("to_lib")?,
                    to_rel: r.try_get("to_rel")?,
                    how: How::from_db(r.try_get("how")?),
                })
            })
            .collect()
    }

    async fn ops_where(&self, sql: &'static str) -> Result<Vec<OpRow>> {
        let rows = sqlx::query(sql).fetch_all(self.reader()).await?;
        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let id: i64 = r.try_get("id")?;
            out.push(OpRow {
                id,
                kind: OpKind::from_db(r.try_get("kind")?),
                state: OpState::from_db(r.try_get("state")?),
                at: r.try_get("at")?,
                steps: self.op_steps(id).await?,
            });
        }
        Ok(out)
    }

    /// The newest file action, whatever its state.
    pub async fn last_op(&self) -> Result<Option<OpRow>> {
        Ok(self
            .ops_where("SELECT id, kind, state, at FROM file_ops ORDER BY id DESC LIMIT 1")
            .await?
            .pop())
    }

    /// File actions a crash interrupted.
    pub async fn planned_ops(&self) -> Result<Vec<OpRow>> {
        self.ops_where(
            "SELECT id, kind, state, at FROM file_ops WHERE state = 'planned' ORDER BY id",
        )
        .await
    }

    /// Whether a model row (present or missing) of `lib` sits at `rel`, inside
    /// it, or around it: moving there would mix two models.
    pub async fn path_taken(&self, lib: LibraryId, rel: &str, except: ModelId) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM models
             WHERE library_id = ?1 AND id != ?2
               AND (rel_path = ?3
                    OR substr(rel_path, 1, length(?3) + 1) = ?3 || '/'
                    OR substr(?3, 1, length(rel_path) + 1) = rel_path || '/')",
        )
        .bind(lib)
        .bind(except)
        .bind(rel)
        .fetch_one(self.reader())
        .await?;
        Ok(n > 0)
    }

    /// Points model `id`'s rows at `to_rel` in `to_lib`, renamed `name`.
    /// Idempotent: rows already moved stay as they are. A model that changes
    /// library gets a new catalogue number there.
    pub async fn relocate_model(
        &self,
        id: ModelId,
        from_rel: &str,
        to_lib: LibraryId,
        to_rel: &str,
        name: &str,
    ) -> Result<()> {
        let now = now_ms();
        let mut tx = self.writer().begin().await?;
        sqlx::query(
            "UPDATE models SET
               number = CASE WHEN library_id = ?1 THEN number ELSE NULL END,
               library_id = ?1, rel_path = ?2, name = ?3, updated_at = ?4
             WHERE id = ?5",
        )
        .bind(to_lib)
        .bind(to_rel)
        .bind(name)
        .bind(now)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        for sql in [
            "UPDATE parts SET
               library_id = ?1, rel_path = ?2 || substr(rel_path, length(?3) + 1), updated_at = ?4,
               path_material = NULL
             WHERE model_id = ?5 AND (rel_path = ?3 OR substr(rel_path, 1, length(?3) + 1) = ?3 || '/')",
            "UPDATE assets SET
               library_id = ?1, rel_path = ?2 || substr(rel_path, length(?3) + 1), updated_at = ?4
             WHERE model_id = ?5 AND (rel_path = ?3 OR substr(rel_path, 1, length(?3) + 1) = ?3 || '/')",
        ] {
            sqlx::query(sql)
            .bind(to_lib)
            .bind(to_rel)
            .bind(from_rel)
            .bind(now)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        self.fill_path_materials().await?;
        self.assign_numbers(to_lib).await?;
        Ok(())
    }

    /// Whether the user wrote anything about model `id` (what a purge or a
    /// trash must not throw away).
    pub async fn has_user_data(&self, id: ModelId) -> Result<bool> {
        let n: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM models m WHERE m.id = ? AND (
               m.notes != '' OR m.source_url IS NOT NULL
               OR EXISTS (SELECT 1 FROM model_tags t WHERE t.model_id = m.id)
               OR EXISTS (SELECT 1 FROM collection_models c WHERE c.model_id = m.id)
               OR EXISTS (SELECT 1 FROM recipes r WHERE r.model_id = m.id)
               OR EXISTS (SELECT 1 FROM attempts a WHERE a.model_id = m.id))",
        )
        .bind(id)
        .fetch_one(self.reader())
        .await?;
        Ok(n > 0)
    }
}
