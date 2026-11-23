//! Missing rows: the grace period before Printdex forgets them, and
//! forgetting on request.

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::ids::{LibraryId, ModelId};

/// How long a missing file or model is kept before a scan forgets it.
pub const MISSING_GRACE_MS: i64 = 30 * 24 * 60 * 60 * 1000;

/// What a purge removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Purged {
    pub models: u64,
    pub parts: u64,
    pub assets: u64,
}

impl Db {
    /// Forgets rows of `lib` missing since before `now - MISSING_GRACE_MS`.
    /// Models the user wrote anything about (notes, source, tags,
    /// collections, recipe, attempts) are never forgotten here; they stay
    /// under the Missing filter until the user forgets them. Run only after
    /// a scan that reached the library folder.
    pub async fn purge_missing(&self, lib: LibraryId, now: i64) -> Result<Purged> {
        let cutoff = now - MISSING_GRACE_MS;
        let mut tx = self.writer().begin().await?;
        let models = sqlx::query!(
            // Safe because only rows without user data, missing for 30 days after a walk that reached the folder.
            "DELETE FROM models
             WHERE library_id = ? AND missing_since IS NOT NULL AND missing_since < ?
               AND notes = '' AND source_url IS NULL
               AND NOT EXISTS (SELECT 1 FROM model_tags t WHERE t.model_id = models.id)
               AND NOT EXISTS (SELECT 1 FROM collection_models c WHERE c.model_id = models.id)
               AND NOT EXISTS (SELECT 1 FROM recipes r WHERE r.model_id = models.id)
               AND NOT EXISTS (SELECT 1 FROM attempts a WHERE a.model_id = models.id)",
            lib,
            cutoff
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        // Files gone from models that are still there (a part deleted from
        // a folder). Missing models keep theirs, to show where they were.
        let parts = sqlx::query!(
            // Safe because only gone files of models that are still there, missing for 30 days.
            "DELETE FROM parts
             WHERE library_id = ? AND missing_since IS NOT NULL AND missing_since < ?
               AND model_id IN (SELECT id FROM models WHERE missing_since IS NULL)",
            lib,
            cutoff
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let assets = sqlx::query!(
            // Safe because only gone files of models that are still there, missing for 30 days.
            "DELETE FROM assets
             WHERE library_id = ? AND missing_since IS NOT NULL AND missing_since < ?
               AND model_id IN (SELECT id FROM models WHERE missing_since IS NULL)",
            lib,
            cutoff
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(Purged {
            models,
            parts,
            assets,
        })
    }

    /// Forgets a missing model and everything recorded about it. Present
    /// models cannot be forgotten.
    pub async fn forget_model(&self, id: ModelId) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        let n = sqlx::query!(
            // Safe because the user asked to forget this missing model.
            "DELETE FROM models WHERE id = ? AND missing_since IS NOT NULL",
            id
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if n == 0 {
            return Err(CoreError::ModelNotFound);
        }
        crate::db::organize::drop_unused_tags(&mut tx).await?;
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Library, LibraryKind, Scope, SearchFilter, Smart, SortOrder};
    use std::path::PathBuf;

    const NOW: i64 = 100 * 24 * 60 * 60 * 1000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    struct Fx {
        _d: tempfile::TempDir,
        db: Db,
        lib: LibraryId,
    }

    async fn fx() -> Fx {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open(&d.path().join("t.db")).await.unwrap();
        let lib = Library {
            id: LibraryId::new(),
            path: PathBuf::from("/lib"),
            display_name: "lib".into(),
            kind: LibraryKind::Local,
            change_mode: crate::db::ChangeMode::Auto,
            sidecar_enabled: true,
            last_full_scan_at: None,
        };
        db.insert_library(&lib).await.unwrap();
        Fx {
            _d: d,
            db,
            lib: lib.id,
        }
    }

    impl Fx {
        /// A model missing for `days` (None: present), with one part.
        async fn model(&self, name: &str, days: Option<i64>) -> ModelId {
            let id = ModelId::new();
            let since = days.map(|d| NOW - d * DAY);
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, missing_since, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'folder', ?, 1, 1)",
            )
            .bind(id)
            .bind(self.lib)
            .bind(name)
            .bind(name)
            .bind(since)
            .execute(self.db.writer())
            .await
            .unwrap();
            self.part(id, &format!("{name}/a.stl"), since).await;
            id
        }

        async fn part(&self, model: ModelId, rel: &str, since: Option<i64>) {
            sqlx::query(
                "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns,
                                    missing_since, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'stl', 1, 1, ?, 1, 1)",
            )
            .bind(crate::ids::PartId::new())
            .bind(model)
            .bind(self.lib)
            .bind(rel)
            .bind(since)
            .execute(self.db.writer())
            .await
            .unwrap();
        }

        async fn exists(&self, id: ModelId) -> bool {
            sqlx::query("SELECT 1 FROM models WHERE id = ?")
                .bind(id)
                .fetch_optional(self.db.writer())
                .await
                .unwrap()
                .is_some()
        }

        async fn parts(&self) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM parts")
                .fetch_one(self.db.writer())
                .await
                .unwrap()
        }
    }

    #[tokio::test]
    async fn missing_rows_purged_after_grace() {
        let f = fx().await;
        let old = f.model("Old", Some(31)).await;
        let recent = f.model("Recent", Some(29)).await;
        let here = f.model("Here", None).await;
        f.part(here, "Here/gone.stl", Some(NOW - 40 * DAY)).await;
        let p = f.db.purge_missing(f.lib, NOW).await.unwrap();
        assert_eq!((p.models, p.parts), (1, 1));
        assert!(!f.exists(old).await);
        assert!(f.exists(recent).await);
        assert!(f.exists(here).await);
        // Recent's part stays with it; Here keeps its present part.
        assert_eq!(f.parts().await, 2);
    }

    #[tokio::test]
    async fn missing_with_user_data_never_purged() {
        let f = fx().await;
        let tagged = f.model("Tagged", Some(400)).await;
        let noted = f.model("Noted", Some(400)).await;
        f.db.add_tag(tagged, "keep").await.unwrap();
        f.db.set_notes(noted, "Glue the hinge").await.unwrap();
        let p = f.db.purge_missing(f.lib, NOW).await.unwrap();
        assert_eq!(p.models, 0);
        assert!(f.exists(tagged).await && f.exists(noted).await);
    }

    #[tokio::test]
    async fn missing_filter_lists_only_missing_models() {
        let f = fx().await;
        let gone = f.model("Gone", Some(3)).await;
        f.model("Here", None).await;
        let hits =
            f.db.search_models_in(
                &Scope::Smart(Smart::Missing),
                "",
                &SearchFilter::default(),
                SortOrder::Name,
                50,
            )
            .await
            .unwrap();
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), [gone]);
        assert!(hits[0].missing);
        assert_eq!(f.db.smart_counts().await.unwrap().missing, 1);
    }

    #[tokio::test]
    async fn forget_removes_missing_model_and_its_tags() {
        let f = fx().await;
        let gone = f.model("Gone", Some(3)).await;
        let here = f.model("Here", None).await;
        f.db.add_tag(gone, "only-on-gone").await.unwrap();
        assert!(matches!(
            f.db.forget_model(here).await,
            Err(CoreError::ModelNotFound)
        ));
        f.db.forget_model(gone).await.unwrap();
        assert!(!f.exists(gone).await);
        assert!(f.db.tags().await.unwrap().is_empty());
    }
}
