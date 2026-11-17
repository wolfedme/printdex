//! Tags, collections, notes and source: what the user writes about a model.
//!
//! Every write bumps the model's `data_updated_at` and refreshes its
//! full-text row in the same transaction.

use sqlx::SqliteConnection;

use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::ids::{CollectionId, ModelId, TagId, Uuid};
use crate::model_data::{
    COLLECTION_MAX_CHARS, CollectionInfo, ModelData, TAG_MAX_CHARS, TEXT_MAX_BYTES, TagCount,
    check_source, clean_name, name_key,
};
use crate::time::now_ms;

fn count(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

/// A cleaned name and its key, or `err` when blank or longer than `max`.
fn checked_name(name: &str, max: usize, err: CoreError) -> Result<(String, String)> {
    let clean = clean_name(name);
    if clean.is_empty() || clean.chars().count() > max {
        return Err(err);
    }
    let key = name_key(&clean);
    Ok((clean, key))
}

/// Fails with `ModelNotFound` unless model `id` exists (missing or not).
pub(crate) async fn ensure_model(conn: &mut SqliteConnection, id: ModelId) -> Result<()> {
    let found = sqlx::query_scalar!(r#"SELECT 1 as "x!: i64" FROM models WHERE id = ?"#, id)
        .fetch_optional(&mut *conn)
        .await?;
    found.map(|_| ()).ok_or(CoreError::ModelNotFound)
}

/// Marks user data of `id` as changed now (`at`).
pub(crate) async fn touch(conn: &mut SqliteConnection, id: ModelId, at: i64) -> Result<()> {
    sqlx::query!("UPDATE models SET data_updated_at = ? WHERE id = ?", at, id)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Rewrites the tags, notes and recipe columns of `id`'s full-text row.
pub(crate) async fn refresh_fts(conn: &mut SqliteConnection, id: ModelId) -> Result<()> {
    let tags: Vec<String> = sqlx::query_scalar!(
        "SELECT t.name FROM model_tags mt JOIN tags t ON t.id = mt.tag_id WHERE mt.model_id = ?",
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let recipe = crate::db::prints::recipe_search_text(conn, id).await?;
    let tags = tags.join(" ");
    sqlx::query!(
        "UPDATE models_fts SET tags = ?, notes = (SELECT notes FROM models WHERE id = ?), recipe = ?
         WHERE rowid = (SELECT rowid FROM models WHERE id = ?)",
        tags,
        id,
        recipe,
        id
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Removes tags no model uses any more.
pub(crate) async fn drop_unused_tags(conn: &mut SqliteConnection) -> Result<()> {
    sqlx::query!("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM model_tags)")
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// Tags `id` with `name`, creating the tag on first use. `false` when the
/// model already had it.
async fn add_tag_in(
    conn: &mut SqliteConnection,
    id: ModelId,
    name: &str,
    now: i64,
) -> Result<bool> {
    let (clean, key) = checked_name(name, TAG_MAX_CHARS, CoreError::TagInvalid)?;
    let new_id = TagId::new();
    sqlx::query!(
        "INSERT OR IGNORE INTO tags (id, name, name_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        new_id,
        clean,
        key,
        now,
        now
    )
    .execute(&mut *conn)
    .await?;
    let tag = sqlx::query_scalar!(
        r#"SELECT id as "id!: TagId" FROM tags WHERE name_key = ?"#,
        key
    )
    .fetch_one(&mut *conn)
    .await?;
    let done = sqlx::query!(
        "INSERT OR IGNORE INTO model_tags (model_id, tag_id, created_at) VALUES (?, ?, ?)",
        id,
        tag,
        now
    )
    .execute(&mut *conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// Puts `id` in the collection named `name`, creating it if needed.
async fn join_collection_named(
    conn: &mut SqliteConnection,
    id: ModelId,
    name: &str,
    now: i64,
) -> Result<()> {
    let (clean, key) = checked_name(name, COLLECTION_MAX_CHARS, CoreError::CollectionNameInvalid)?;
    let new_id = CollectionId::new();
    sqlx::query!(
        "INSERT OR IGNORE INTO collections (id, name, name_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        new_id,
        clean,
        key,
        now,
        now
    )
    .execute(&mut *conn)
    .await?;
    let coll = sqlx::query_scalar!(
        r#"SELECT id as "id!: CollectionId" FROM collections WHERE name_key = ?"#,
        key
    )
    .fetch_one(&mut *conn)
    .await?;
    join_collection(conn, coll, id, now).await?;
    Ok(())
}

async fn join_collection(
    conn: &mut SqliteConnection,
    coll: CollectionId,
    id: ModelId,
    now: i64,
) -> Result<bool> {
    let done = sqlx::query!(
        "INSERT OR IGNORE INTO collection_models (collection_id, model_id, position, created_at)
         VALUES (?1, ?2, (SELECT coalesce(max(position), 0) + 1 FROM collection_models WHERE collection_id = ?1), ?3)",
        coll,
        id,
        now
    )
    .execute(&mut *conn)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The organizing part of a model's data, as a sidecar carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Organize {
    pub notes: String,
    pub source: Option<String>,
    pub tags: Vec<String>,
    pub collections: Vec<String>,
    pub updated_at: i64,
}

impl Db {
    /// This install's id, created on first use.
    pub async fn install_id(&self) -> Result<Uuid> {
        let fresh = Uuid::now_v7().hyphenated().to_string();
        sqlx::query!(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('install_id', ?)",
            fresh
        )
        .execute(self.writer())
        .await?;
        let v = sqlx::query_scalar!("SELECT value FROM meta WHERE key = 'install_id'")
            .fetch_one(self.writer())
            .await?;
        Uuid::parse_str(&v).map_err(|e| CoreError::Db(sqlx::Error::Decode(Box::new(e))))
    }

    /// Adds tag `name` to model `id`. `false` when it was already there.
    pub async fn add_tag(&self, id: ModelId, name: &str) -> Result<bool> {
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        let now = now_ms();
        let added = add_tag_in(&mut tx, id, name, now).await?;
        if added {
            touch(&mut tx, id, now).await?;
            refresh_fts(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(added)
    }

    /// Removes tag `name` (matched ignoring case) from model `id`. `false`
    /// when the model did not have it.
    pub async fn remove_tag(&self, id: ModelId, name: &str) -> Result<bool> {
        let key = name_key(name);
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        let done = sqlx::query!(
            "DELETE FROM model_tags WHERE model_id = ? AND tag_id = (SELECT id FROM tags WHERE name_key = ?)",
            id,
            key
        )
        .execute(&mut *tx)
        .await?;
        let removed = done.rows_affected() > 0;
        if removed {
            drop_unused_tags(&mut tx).await?;
            touch(&mut tx, id, now_ms()).await?;
            refresh_fts(&mut tx, id).await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    /// Tags used by present models, most used first, then by name.
    pub async fn tags(&self) -> Result<Vec<TagCount>> {
        let rows = sqlx::query!(
            r#"SELECT t.name, count(*) as "n!: i64" FROM tags t
               JOIN model_tags mt ON mt.tag_id = t.id
               JOIN models m ON m.id = mt.model_id AND m.missing_since IS NULL
               GROUP BY t.id ORDER BY 2 DESC, t.name COLLATE NOCASE"#
        )
        .fetch_all(self.reader())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| TagCount {
                name: r.name,
                count: count(r.n),
            })
            .collect())
    }

    /// All collections by name, with their present-model counts.
    pub async fn collections(&self) -> Result<Vec<CollectionInfo>> {
        let rows = sqlx::query!(
            r#"SELECT c.id as "id!: CollectionId", c.name,
                      (SELECT count(*) FROM collection_models cm JOIN models m ON m.id = cm.model_id
                        WHERE cm.collection_id = c.id AND m.missing_since IS NULL) as "n!: i64"
               FROM collections c ORDER BY c.name COLLATE NOCASE"#
        )
        .fetch_all(self.reader())
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| CollectionInfo {
                id: r.id,
                name: r.name,
                count: count(r.n),
            })
            .collect())
    }

    /// Creates an empty collection.
    pub async fn create_collection(&self, name: &str) -> Result<CollectionInfo> {
        let (clean, key) =
            checked_name(name, COLLECTION_MAX_CHARS, CoreError::CollectionNameInvalid)?;
        let id = CollectionId::new();
        let now = now_ms();
        let done = sqlx::query!(
            "INSERT OR IGNORE INTO collections (id, name, name_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
            id,
            clean,
            key,
            now,
            now
        )
        .execute(self.writer())
        .await?;
        if done.rows_affected() == 0 {
            return Err(CoreError::CollectionExists);
        }
        Ok(CollectionInfo {
            id,
            name: clean,
            count: 0,
        })
    }

    /// Models in collection `id` (missing or not).
    pub async fn collection_members(&self, id: CollectionId) -> Result<Vec<ModelId>> {
        Ok(sqlx::query_scalar!(
            r#"SELECT model_id as "m!: ModelId" FROM collection_models WHERE collection_id = ?"#,
            id
        )
        .fetch_all(self.writer())
        .await?)
    }

    /// Renames collection `id`. Its members' data counts as changed (their
    /// sidecars name the collection).
    pub async fn rename_collection(&self, id: CollectionId, name: &str) -> Result<()> {
        let (clean, key) =
            checked_name(name, COLLECTION_MAX_CHARS, CoreError::CollectionNameInvalid)?;
        let mut tx = self.writer().begin().await?;
        let taken = sqlx::query_scalar!(
            r#"SELECT id as "id!: CollectionId" FROM collections WHERE name_key = ?"#,
            key
        )
        .fetch_optional(&mut *tx)
        .await?;
        if taken.is_some_and(|other| other != id) {
            return Err(CoreError::CollectionExists);
        }
        let now = now_ms();
        let done = sqlx::query!(
            "UPDATE collections SET name = ?, name_key = ?, updated_at = ? WHERE id = ?",
            clean,
            key,
            now,
            id
        )
        .execute(&mut *tx)
        .await?;
        if done.rows_affected() == 0 {
            return Err(CoreError::CollectionNotFound);
        }
        sqlx::query!(
            "UPDATE models SET data_updated_at = ? WHERE id IN (SELECT model_id FROM collection_models WHERE collection_id = ?)",
            now,
            id
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Deletes collection `id`; models stay.
    pub async fn delete_collection(&self, id: CollectionId) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        let now = now_ms();
        sqlx::query!(
            "UPDATE models SET data_updated_at = ? WHERE id IN (SELECT model_id FROM collection_models WHERE collection_id = ?)",
            now,
            id
        )
        .execute(&mut *tx)
        .await?;
        let done = sqlx::query!("DELETE FROM collections WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        if done.rows_affected() == 0 {
            return Err(CoreError::CollectionNotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    /// Adds model `model` to the end of collection `id`. `false` when it was
    /// already in it.
    pub async fn add_to_collection(&self, id: CollectionId, model: ModelId) -> Result<bool> {
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, model).await?;
        let exists =
            sqlx::query_scalar!(r#"SELECT 1 as "x!: i64" FROM collections WHERE id = ?"#, id)
                .fetch_optional(&mut *tx)
                .await?;
        if exists.is_none() {
            return Err(CoreError::CollectionNotFound);
        }
        let now = now_ms();
        let added = join_collection(&mut tx, id, model, now).await?;
        if added {
            touch(&mut tx, model, now).await?;
        }
        tx.commit().await?;
        Ok(added)
    }

    /// Takes model `model` out of collection `id`.
    pub async fn remove_from_collection(&self, id: CollectionId, model: ModelId) -> Result<bool> {
        let mut tx = self.writer().begin().await?;
        let done = sqlx::query!(
            "DELETE FROM collection_models WHERE collection_id = ? AND model_id = ?",
            id,
            model
        )
        .execute(&mut *tx)
        .await?;
        let removed = done.rows_affected() > 0;
        if removed {
            touch(&mut tx, model, now_ms()).await?;
        }
        tx.commit().await?;
        Ok(removed)
    }

    /// Sets the Markdown notes of model `id`.
    pub async fn set_notes(&self, id: ModelId, notes: &str) -> Result<()> {
        if notes.len() > TEXT_MAX_BYTES {
            return Err(CoreError::TextTooLong);
        }
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        sqlx::query!("UPDATE models SET notes = ? WHERE id = ?", notes, id)
            .execute(&mut *tx)
            .await?;
        touch(&mut tx, id, now_ms()).await?;
        refresh_fts(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Sets (or clears, with `None` or blank) the source link of model `id`.
    pub async fn set_source(&self, id: ModelId, url: Option<&str>) -> Result<()> {
        let url = url.map(str::trim).filter(|u| !u.is_empty());
        if let Some(u) = url {
            check_source(u)?;
        }
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        sqlx::query!("UPDATE models SET source_url = ? WHERE id = ?", url, id)
            .execute(&mut *tx)
            .await?;
        touch(&mut tx, id, now_ms()).await?;
        tx.commit().await?;
        Ok(())
    }

    /// The organizing data of model `id`, as a sidecar mirrors it.
    pub async fn organize(&self, id: ModelId) -> Result<Option<Organize>> {
        let Some(m) = sqlx::query!(
            r#"SELECT notes, source_url, data_updated_at FROM models WHERE id = ?"#,
            id
        )
        .fetch_optional(self.writer())
        .await?
        else {
            return Ok(None);
        };
        let tags = sqlx::query_scalar!(
            "SELECT t.name FROM model_tags mt JOIN tags t ON t.id = mt.tag_id
             WHERE mt.model_id = ? ORDER BY t.name COLLATE NOCASE",
            id
        )
        .fetch_all(self.writer())
        .await?;
        let collections = sqlx::query_scalar!(
            "SELECT c.name FROM collection_models cm JOIN collections c ON c.id = cm.collection_id
             WHERE cm.model_id = ? ORDER BY c.name COLLATE NOCASE",
            id
        )
        .fetch_all(self.writer())
        .await?;
        Ok(Some(Organize {
            notes: m.notes,
            source: m.source_url,
            tags,
            collections,
            updated_at: m.data_updated_at.unwrap_or(0),
        }))
    }

    /// Replaces the organizing data of model `id` (sidecar import), keeping
    /// `data.updated_at` as the change time.
    pub async fn import_organize(&self, id: ModelId, data: &Organize) -> Result<()> {
        if data.notes.len() > TEXT_MAX_BYTES {
            return Err(CoreError::TextTooLong);
        }
        let source = data
            .source
            .as_deref()
            .map(str::trim)
            .filter(|u| check_source(u).is_ok());
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        sqlx::query!(
            "UPDATE models SET notes = ?, source_url = ?, data_updated_at = ? WHERE id = ?",
            data.notes,
            source,
            data.updated_at,
            id
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query!("DELETE FROM model_tags WHERE model_id = ?", id)
            .execute(&mut *tx)
            .await?;
        for tag in data.tags.iter().take(200) {
            // A bad name in a hand-edited sidecar is skipped, not fatal.
            if let Err(e) = add_tag_in(&mut tx, id, tag, data.updated_at).await
                && !matches!(e, CoreError::TagInvalid)
            {
                return Err(e);
            }
        }
        drop_unused_tags(&mut tx).await?;
        sqlx::query!("DELETE FROM collection_models WHERE model_id = ?", id)
            .execute(&mut *tx)
            .await?;
        for name in data.collections.iter().take(200) {
            if let Err(e) = join_collection_named(&mut tx, id, name, data.updated_at).await
                && !matches!(e, CoreError::CollectionNameInvalid)
            {
                return Err(e);
            }
        }
        refresh_fts(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Everything the user wrote about model `id`; `None` when there is no
    /// such model.
    pub async fn model_data(&self, id: ModelId) -> Result<Option<ModelData>> {
        let Some(m) = sqlx::query!(r#"SELECT notes, source_url FROM models WHERE id = ?"#, id)
            .fetch_optional(self.reader())
            .await?
        else {
            return Ok(None);
        };
        let tags = sqlx::query_scalar!(
            "SELECT t.name FROM model_tags mt JOIN tags t ON t.id = mt.tag_id
             WHERE mt.model_id = ? ORDER BY t.name COLLATE NOCASE",
            id
        )
        .fetch_all(self.reader())
        .await?;
        let collections = sqlx::query!(
            r#"SELECT c.id as "id!: CollectionId", c.name,
                      (SELECT count(*) FROM collection_models x JOIN models m ON m.id = x.model_id
                        WHERE x.collection_id = c.id AND m.missing_since IS NULL) as "n!: i64"
               FROM collection_models cm JOIN collections c ON c.id = cm.collection_id
               WHERE cm.model_id = ? ORDER BY c.name COLLATE NOCASE"#,
            id
        )
        .fetch_all(self.reader())
        .await?
        .into_iter()
        .map(|r| CollectionInfo {
            id: r.id,
            name: r.name,
            count: count(r.n),
        })
        .collect();
        let mut conn = self.reader().acquire().await?;
        let recipe = crate::db::prints::load_recipe(&mut conn, id).await?;
        let attempts = crate::db::prints::load_attempts(&mut conn, id, self.photo_dir()).await?;
        Ok(Some(ModelData {
            notes: m.notes,
            source: m.source_url,
            tags,
            collections,
            recipe,
            attempts,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Library, LibraryKind};
    use crate::ids::LibraryId;
    use std::path::PathBuf;

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
        async fn model(&self, name: &str) -> ModelId {
            let id = ModelId::new();
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'folder', 1, 1)",
            )
            .bind(id)
            .bind(self.lib)
            .bind(name)
            .bind(name)
            .execute(self.db.writer())
            .await
            .unwrap();
            id
        }

        async fn names(&self, q: &str) -> Vec<String> {
            self.db
                .search_models(q, 50)
                .await
                .unwrap()
                .into_iter()
                .map(|h| h.name)
                .collect()
        }
    }

    #[tokio::test]
    async fn tags_match_case_insensitively() {
        let f = fx().await;
        let a = f.model("A").await;
        let b = f.model("B").await;
        assert!(f.db.add_tag(a, "PETG").await.unwrap());
        assert!(!f.db.add_tag(a, " petg ").await.unwrap());
        assert!(f.db.add_tag(b, "Petg").await.unwrap());
        f.db.add_tag(b, "Über").await.unwrap();
        assert!(!f.db.add_tag(b, "über").await.unwrap());
        let tags = f.db.tags().await.unwrap();
        assert_eq!(
            tags,
            vec![
                TagCount {
                    name: "PETG".into(),
                    count: 2
                },
                TagCount {
                    name: "Über".into(),
                    count: 1
                }
            ]
        );
        assert_eq!(
            f.db.model_data(b).await.unwrap().unwrap().tags,
            ["PETG", "Über"]
        );
    }

    #[tokio::test]
    async fn unused_tag_is_deleted() {
        let f = fx().await;
        let a = f.model("A").await;
        f.db.add_tag(a, "functional").await.unwrap();
        assert!(f.db.remove_tag(a, "FUNCTIONAL").await.unwrap());
        assert!(!f.db.remove_tag(a, "functional").await.unwrap());
        assert!(f.db.tags().await.unwrap().is_empty());
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM tags")
            .fetch_one(f.db.writer())
            .await
            .unwrap();
        assert_eq!(n, 0);
    }

    #[tokio::test]
    async fn tag_too_long_rejected() {
        let f = fx().await;
        let a = f.model("A").await;
        let long = "x".repeat(65);
        assert!(matches!(
            f.db.add_tag(a, &long).await,
            Err(CoreError::TagInvalid)
        ));
        assert!(matches!(
            f.db.add_tag(a, "   ").await,
            Err(CoreError::TagInvalid)
        ));
        assert!(matches!(
            f.db.add_tag(ModelId::new(), "x").await,
            Err(CoreError::ModelNotFound)
        ));
    }

    #[tokio::test]
    async fn collection_names_unique() {
        let f = fx().await;
        let c = f.db.create_collection(" Desk  organizers ").await.unwrap();
        assert_eq!(c.name, "Desk organizers");
        assert!(matches!(
            f.db.create_collection("desk ORGANIZERS").await,
            Err(CoreError::CollectionExists)
        ));
        let d = f.db.create_collection("Gifts").await.unwrap();
        assert!(matches!(
            f.db.rename_collection(d.id, "Desk organizers").await,
            Err(CoreError::CollectionExists)
        ));
        f.db.rename_collection(c.id, "Desk Organizers")
            .await
            .unwrap();
        let names: Vec<_> =
            f.db.collections()
                .await
                .unwrap()
                .into_iter()
                .map(|c| c.name)
                .collect();
        assert_eq!(names, ["Desk Organizers", "Gifts"]);
    }

    #[tokio::test]
    async fn delete_collection_keeps_models() {
        let f = fx().await;
        let a = f.model("A").await;
        let c = f.db.create_collection("Gifts").await.unwrap();
        assert!(f.db.add_to_collection(c.id, a).await.unwrap());
        assert!(!f.db.add_to_collection(c.id, a).await.unwrap());
        assert_eq!(f.db.collections().await.unwrap()[0].count, 1);
        f.db.delete_collection(c.id).await.unwrap();
        assert!(f.db.collections().await.unwrap().is_empty());
        assert_eq!(f.names("").await, ["A"]);
        assert!(matches!(
            f.db.delete_collection(c.id).await,
            Err(CoreError::CollectionNotFound)
        ));
    }

    #[tokio::test]
    async fn notes_and_tags_are_searchable() {
        let f = fx().await;
        let a = f.model("Bin").await;
        f.model("Hook").await;
        f.db.add_tag(a, "gridfinity").await.unwrap();
        f.db.set_notes(a, "Warps unless the bed is hot.")
            .await
            .unwrap();
        assert_eq!(f.names("gridfin").await, ["Bin"]);
        assert_eq!(f.names("warps").await, ["Bin"]);
        assert_eq!(f.names("bin gridfinity").await, ["Bin"]);
        f.db.remove_tag(a, "gridfinity").await.unwrap();
        assert!(f.names("gridfinity").await.is_empty());
    }

    #[tokio::test]
    async fn source_is_checked() {
        let f = fx().await;
        let a = f.model("A").await;
        assert!(matches!(
            f.db.set_source(a, Some("ftp://x")).await,
            Err(CoreError::SourceInvalid)
        ));
        f.db.set_source(a, Some(" https://printables.com/model/1 "))
            .await
            .unwrap();
        let d = f.db.model_data(a).await.unwrap().unwrap();
        assert_eq!(d.source.as_deref(), Some("https://printables.com/model/1"));
        f.db.set_source(a, Some("")).await.unwrap();
        assert_eq!(f.db.model_data(a).await.unwrap().unwrap().source, None);
    }

    #[tokio::test]
    async fn import_replaces_tags_and_collections() {
        let f = fx().await;
        let a = f.model("A").await;
        f.db.add_tag(a, "old").await.unwrap();
        let data = Organize {
            notes: "hi".into(),
            source: Some("javascript:alert(1)".into()),
            tags: vec!["new".into(), "".into()],
            collections: vec!["Gifts".into()],
            updated_at: 1234,
        };
        f.db.import_organize(a, &data).await.unwrap();
        let back = f.db.organize(a).await.unwrap().unwrap();
        assert_eq!(back.tags, ["new"]);
        assert_eq!(back.collections, ["Gifts"]);
        assert_eq!(back.source, None);
        assert_eq!(back.updated_at, 1234);
        assert_eq!(f.db.tags().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn migration_keeps_existing_models_searchable() {
        use sqlx::sqlite::SqliteConnectOptions;
        use std::str::FromStr;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("old.db");
        {
            let pool = sqlx::SqlitePool::connect_with(
                SqliteConnectOptions::from_str("sqlite://")
                    .unwrap()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
            let mut m = sqlx::migrate!("./migrations");
            m.migrations = std::borrow::Cow::Owned(m.migrations[..2].to_vec());
            m.run(&pool).await.unwrap();
            sqlx::query(
                "INSERT INTO libraries (id, path, display_name, created_at, updated_at)
                 VALUES (?, '/l', 'l', 1, 1)",
            )
            .bind(LibraryId::new())
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
                 VALUES (?, (SELECT id FROM libraries), 'Dragons/Red', 'Red Dragon', 'folder', 1, 1)",
            )
            .bind(ModelId::new())
            .execute(&pool)
            .await
            .unwrap();
            pool.close().await;
        }
        let db = Db::open(&path).await.unwrap();
        let hits = db.search_models("dragons", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "Red Dragon");
    }

    #[tokio::test]
    async fn install_id_is_stable() {
        let f = fx().await;
        let a = f.db.install_id().await.unwrap();
        assert_eq!(a, f.db.install_id().await.unwrap());
        assert_eq!(a.get_version_num(), 7);
    }
}
