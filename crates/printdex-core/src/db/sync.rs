//! Sync tables (migration 0008): spaces, links, id maps, field stamps,
//! cursors, devices and "Edited elsewhere" history.
//!
//! Runtime-checked queries (`sqlx::query`), like the rest of the sync
//! module, so adding sync needs no offline query cache entries.

use std::collections::BTreeMap;

use sqlx::Row;
use uuid::Uuid;

use crate::db::Db;
use crate::error::Result;
use crate::ids::{CollectionId, LibraryId, ModelId};
use crate::sync::Stamp;

/// What kind of place a space is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceKind {
    /// `<library root>/.printdex/sync`, for one shared library.
    Library,
    /// A folder or remote target, for any number of libraries.
    Target,
}

impl SpaceKind {
    fn as_str(self) -> &'static str {
        match self {
            SpaceKind::Library => "library",
            SpaceKind::Target => "target",
        }
    }

    fn parse(s: &str) -> SpaceKind {
        if s == "library" {
            SpaceKind::Library
        } else {
            SpaceKind::Target
        }
    }
}

/// A space as stored.
#[derive(Clone, Debug, PartialEq)]
pub struct SpaceRow {
    pub id: i64,
    pub kind: SpaceKind,
    pub library_id: Option<LibraryId>,
    /// The target as JSON (target spaces).
    pub target: Option<String>,
    pub name: String,
    pub space_id: Uuid,
    pub encrypted: bool,
    pub next_seq: u64,
    pub snap_seq: u64,
    pub since_snap: u64,
    pub last_sync_at: Option<i64>,
    pub last_error: Option<String>,
}

/// A key's winning stamp and value.
#[derive(Clone, Debug, PartialEq)]
pub struct FieldRow {
    pub key: String,
    /// The model sync id the key belongs to.
    pub model: Option<Uuid>,
    pub stamp: Stamp,
    pub seq: u64,
    pub hash: u64,
    pub value: serde_json::Value,
}

/// Another install seen in a space.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceRow {
    pub install: Uuid,
    pub name: String,
    pub app_version: String,
    pub last_seen: i64,
}

/// A kept losing value of a concurrent edit.
#[derive(Clone, Debug, PartialEq)]
pub struct EditEntry {
    pub id: Uuid,
    pub model: ModelId,
    /// Which field (a sync key's text).
    pub key: String,
    pub value: serde_json::Value,
    pub device: String,
    pub at: i64,
}

fn u(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

fn i(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

fn space_row(r: &sqlx::sqlite::SqliteRow) -> SpaceRow {
    SpaceRow {
        id: r.get("id"),
        kind: SpaceKind::parse(r.get::<String, _>("kind").as_str()),
        library_id: r.get("library_id"),
        target: r.get("target"),
        name: r.get("name"),
        space_id: r.get("space_id"),
        encrypted: r.get::<i64, _>("encrypted") != 0,
        next_seq: u(r.get("next_seq")),
        snap_seq: u(r.get("snap_seq")),
        since_snap: u(r.get("since_snap")),
        last_sync_at: r.get("last_sync_at"),
        last_error: r.get("last_error"),
    }
}

fn field_row(r: &sqlx::sqlite::SqliteRow) -> Option<FieldRow> {
    Some(FieldRow {
        key: r.get("key"),
        model: r.get("model"),
        stamp: serde_json::from_str(r.get::<&str, _>("stamp")).ok()?,
        seq: u(r.get("seq")),
        hash: r.get::<i64, _>("hash") as u64,
        value: serde_json::from_str(r.get::<&str, _>("value")).ok()?,
    })
}

impl Db {
    pub(crate) async fn sync_meta(&self, key: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT value FROM sync_meta WHERE key = ?")
                .bind(key)
                .fetch_optional(self.writer())
                .await?,
        )
    }

    pub(crate) async fn set_sync_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO sync_meta (key, value) VALUES (?, ?) ON CONFLICT (key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(value)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    // ---- spaces ----

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_space(
        &self,
        kind: SpaceKind,
        library: Option<LibraryId>,
        target: Option<&str>,
        name: &str,
        space_id: Uuid,
        encrypted: bool,
        now: i64,
    ) -> Result<i64> {
        let id = sqlx::query(
            "INSERT INTO sync_spaces (kind, library_id, target, name, space_id, encrypted, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(kind.as_str())
        .bind(library)
        .bind(target)
        .bind(name)
        .bind(space_id)
        .bind(i64::from(encrypted))
        .bind(now)
        .execute(self.writer())
        .await?
        .last_insert_rowid();
        Ok(id)
    }

    pub(crate) async fn sync_spaces(&self) -> Result<Vec<SpaceRow>> {
        let rows = sqlx::query("SELECT * FROM sync_spaces ORDER BY id")
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().map(space_row).collect())
    }

    pub(crate) async fn sync_space(&self, id: i64) -> Result<Option<SpaceRow>> {
        let row = sqlx::query("SELECT * FROM sync_spaces WHERE id = ?")
            .bind(id)
            .fetch_optional(self.writer())
            .await?;
        Ok(row.as_ref().map(space_row))
    }

    pub(crate) async fn remove_space(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM sync_spaces WHERE id = ?")
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// After a batch was written: the next seq and the records since the
    /// last snapshot.
    pub(crate) async fn set_space_seq(
        &self,
        id: i64,
        next_seq: u64,
        since_snap: u64,
    ) -> Result<()> {
        sqlx::query("UPDATE sync_spaces SET next_seq = ?, since_snap = ? WHERE id = ?")
            .bind(i(next_seq))
            .bind(i(since_snap))
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    pub(crate) async fn set_space_snap(&self, id: i64, snap_seq: u64) -> Result<()> {
        sqlx::query("UPDATE sync_spaces SET snap_seq = ?, since_snap = 0 WHERE id = ?")
            .bind(i(snap_seq))
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    pub(crate) async fn set_space_result(
        &self,
        id: i64,
        at: Option<i64>,
        error: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE sync_spaces SET last_sync_at = coalesce(?, last_sync_at), last_error = ? WHERE id = ?",
        )
        .bind(at)
        .bind(error)
        .bind(id)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    // ---- links ----

    pub(crate) async fn link_library(
        &self,
        space: i64,
        lib: LibraryId,
        lib_sync: Uuid,
    ) -> Result<()> {
        sqlx::query(
            "INSERT OR REPLACE INTO sync_links (space, library_id, lib_sync_id) VALUES (?, ?, ?)",
        )
        .bind(space)
        .bind(lib)
        .bind(lib_sync)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    pub(crate) async fn unlink_library(&self, space: i64, lib: LibraryId) -> Result<bool> {
        let done = sqlx::query("DELETE FROM sync_links WHERE space = ? AND library_id = ?")
            .bind(space)
            .bind(lib)
            .execute(self.writer())
            .await?;
        Ok(done.rows_affected() > 0)
    }

    /// The libraries linked in `space`: local id → library sync id.
    pub(crate) async fn sync_links(&self, space: i64) -> Result<Vec<(LibraryId, Uuid)>> {
        let rows = sqlx::query("SELECT library_id, lib_sync_id FROM sync_links WHERE space = ?")
            .bind(space)
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Spaces `lib` is linked in.
    pub(crate) async fn spaces_of_library(&self, lib: LibraryId) -> Result<Vec<i64>> {
        Ok(
            sqlx::query_scalar("SELECT space FROM sync_links WHERE library_id = ?")
                .bind(lib)
                .fetch_all(self.writer())
                .await?,
        )
    }

    // ---- model and collection ids ----

    /// The sync id model `model` publishes under, creating one (its own
    /// id) on first use.
    pub(crate) async fn model_sync_id(
        &self,
        space: i64,
        lib_sync: Uuid,
        model: ModelId,
    ) -> Result<Uuid> {
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT sync_id FROM sync_models
             WHERE space = ? AND lib_sync_id = ? AND model_id = ? AND alias_of IS NULL
             ORDER BY sync_id LIMIT 1",
        )
        .bind(space)
        .bind(lib_sync)
        .bind(model)
        .fetch_optional(self.writer())
        .await?;
        if let Some(id) = found {
            return Ok(id);
        }
        sqlx::query(
            "INSERT INTO sync_models (space, lib_sync_id, sync_id, model_id) VALUES (?, ?, ?, ?)
             ON CONFLICT DO UPDATE SET model_id = excluded.model_id",
        )
        .bind(space)
        .bind(lib_sync)
        .bind(model.0)
        .bind(model)
        .execute(self.writer())
        .await?;
        Ok(model.0)
    }

    /// The local model a sync id stands for and the id it is an alias of.
    /// `None`: never seen.
    pub(crate) async fn sync_model(
        &self,
        space: i64,
        lib_sync: Uuid,
        sync_id: Uuid,
    ) -> Result<Option<(Option<ModelId>, Option<Uuid>)>> {
        let row = sqlx::query(
            "SELECT model_id, alias_of FROM sync_models WHERE space = ? AND lib_sync_id = ? AND sync_id = ?",
        )
        .bind(space)
        .bind(lib_sync)
        .bind(sync_id)
        .fetch_optional(self.writer())
        .await?;
        Ok(row.map(|r| (r.get(0), r.get(1))))
    }

    /// Follows aliases to the winning sync id.
    pub(crate) async fn resolve_sync_id(
        &self,
        space: i64,
        lib_sync: Uuid,
        sync_id: Uuid,
    ) -> Result<Uuid> {
        let mut id = sync_id;
        for _ in 0..8 {
            match self.sync_model(space, lib_sync, id).await? {
                Some((_, Some(next))) if next != id => id = next,
                _ => break,
            }
        }
        Ok(id)
    }

    pub(crate) async fn map_model(
        &self,
        space: i64,
        lib_sync: Uuid,
        sync_id: Uuid,
        model: Option<ModelId>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO sync_models (space, lib_sync_id, sync_id, model_id) VALUES (?, ?, ?, ?)
             ON CONFLICT DO UPDATE SET model_id = coalesce(excluded.model_id, model_id)",
        )
        .bind(space)
        .bind(lib_sync)
        .bind(sync_id)
        .bind(model)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    pub(crate) async fn set_alias(
        &self,
        space: i64,
        lib_sync: Uuid,
        loser: Uuid,
        winner: Uuid,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO sync_models (space, lib_sync_id, sync_id, alias_of) VALUES (?, ?, ?, ?)
             ON CONFLICT DO UPDATE SET alias_of = excluded.alias_of",
        )
        .bind(space)
        .bind(lib_sync)
        .bind(loser)
        .bind(winner)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Sync ids (not aliases) whose model isn't on this computer.
    pub(crate) async fn parked(&self, space: i64) -> Result<Vec<(Uuid, Uuid)>> {
        let rows = sqlx::query(
            "SELECT lib_sync_id, sync_id FROM sync_models
             WHERE space = ? AND model_id IS NULL AND alias_of IS NULL",
        )
        .bind(space)
        .fetch_all(self.writer())
        .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// Model sync ids already mapped to local models of a library.
    pub(crate) async fn mapped_models(
        &self,
        space: i64,
        lib_sync: Uuid,
    ) -> Result<Vec<(Uuid, ModelId)>> {
        let rows = sqlx::query(
            "SELECT sync_id, model_id FROM sync_models
             WHERE space = ? AND lib_sync_id = ? AND model_id IS NOT NULL AND alias_of IS NULL",
        )
        .bind(space)
        .bind(lib_sync)
        .fetch_all(self.writer())
        .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    /// The sync id collection `id` publishes under (its own id at first).
    pub(crate) async fn collection_sync_id(&self, space: i64, id: CollectionId) -> Result<Uuid> {
        let found: Option<Uuid> = sqlx::query_scalar(
            "SELECT sync_id FROM sync_collections WHERE space = ? AND collection_id = ? ORDER BY sync_id LIMIT 1",
        )
        .bind(space)
        .bind(id)
        .fetch_optional(self.writer())
        .await?;
        if let Some(s) = found {
            return Ok(s);
        }
        self.map_collection(space, id.0, Some(id)).await?;
        Ok(id.0)
    }

    pub(crate) async fn local_collection(
        &self,
        space: i64,
        sync_id: Uuid,
    ) -> Result<Option<CollectionId>> {
        Ok(sqlx::query_scalar(
            "SELECT collection_id FROM sync_collections WHERE space = ? AND sync_id = ?",
        )
        .bind(space)
        .bind(sync_id)
        .fetch_optional(self.writer())
        .await?
        .flatten())
    }

    pub(crate) async fn map_collection(
        &self,
        space: i64,
        sync_id: Uuid,
        id: Option<CollectionId>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO sync_collections (space, sync_id, collection_id) VALUES (?, ?, ?)
             ON CONFLICT DO UPDATE SET collection_id = excluded.collection_id",
        )
        .bind(space)
        .bind(sync_id)
        .bind(id)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    /// Every collection sync id known in `space`.
    pub(crate) async fn collection_maps(
        &self,
        space: i64,
    ) -> Result<Vec<(Uuid, Option<CollectionId>)>> {
        let rows =
            sqlx::query("SELECT sync_id, collection_id FROM sync_collections WHERE space = ?")
                .bind(space)
                .fetch_all(self.writer())
                .await?;
        Ok(rows.iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    // ---- fields ----

    pub(crate) async fn sync_field(&self, space: i64, key: &str) -> Result<Option<FieldRow>> {
        let row = sqlx::query("SELECT * FROM sync_fields WHERE space = ? AND key = ?")
            .bind(space)
            .bind(key)
            .fetch_optional(self.writer())
            .await?;
        Ok(row.as_ref().and_then(field_row))
    }

    /// Fields of one model sync id.
    pub(crate) async fn model_fields(&self, space: i64, model: Uuid) -> Result<Vec<FieldRow>> {
        let rows = sqlx::query("SELECT * FROM sync_fields WHERE space = ? AND model = ?")
            .bind(space)
            .bind(model)
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().filter_map(field_row).collect())
    }

    /// Fields that belong to no model (collections).
    pub(crate) async fn space_fields(&self, space: i64) -> Result<Vec<FieldRow>> {
        let rows = sqlx::query("SELECT * FROM sync_fields WHERE space = ? AND model IS NULL")
            .bind(space)
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().filter_map(field_row).collect())
    }

    /// Every field of `space` (snapshots).
    pub(crate) async fn all_fields(&self, space: i64) -> Result<Vec<FieldRow>> {
        let rows = sqlx::query("SELECT * FROM sync_fields WHERE space = ?")
            .bind(space)
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().filter_map(field_row).collect())
    }

    /// Rows whose key text starts with `prefix` (identities of a library).
    pub(crate) async fn space_identity_rows(
        &self,
        space: i64,
        prefix: &str,
    ) -> Result<Vec<FieldRow>> {
        let pattern = format!("{}%", prefix.replace('%', "").replace('_', "\\_"));
        let rows =
            sqlx::query("SELECT * FROM sync_fields WHERE space = ? AND key LIKE ? ESCAPE '\\'")
                .bind(space)
                .bind(pattern)
                .fetch_all(self.writer())
                .await?;
        Ok(rows.iter().filter_map(field_row).collect())
    }

    pub(crate) async fn put_fields(&self, space: i64, rows: &[FieldRow]) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        for r in rows {
            sqlx::query(
                "INSERT INTO sync_fields (space, key, model, stamp, seq, hash, value)
                 VALUES (?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (space, key) DO UPDATE SET model = excluded.model, stamp = excluded.stamp,
                   seq = excluded.seq, hash = excluded.hash, value = excluded.value",
            )
            .bind(space)
            .bind(&r.key)
            .bind(r.model)
            .bind(serde_json::to_string(&r.stamp).unwrap_or_default())
            .bind(i(r.seq))
            .bind(r.hash as i64)
            .bind(serde_json::to_string(&r.value).unwrap_or_else(|_| "null".into()))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn delete_fields(&self, space: i64, keys: &[String]) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        for k in keys {
            sqlx::query("DELETE FROM sync_fields WHERE space = ? AND key = ?")
                .bind(space)
                .bind(k)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---- dirty set ----

    pub(crate) async fn mark_sync_dirty(&self, ids: &[ModelId]) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        for id in ids {
            sqlx::query("INSERT OR IGNORE INTO sync_dirty (model_id) VALUES (?)")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn sync_dirty(&self) -> Result<Vec<ModelId>> {
        Ok(sqlx::query_scalar("SELECT model_id FROM sync_dirty")
            .fetch_all(self.writer())
            .await?)
    }

    pub(crate) async fn clear_sync_dirty(&self, ids: &[ModelId]) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        for id in ids {
            sqlx::query("DELETE FROM sync_dirty WHERE model_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    // ---- cursors and devices ----

    pub(crate) async fn sync_cursors(&self, space: i64) -> Result<BTreeMap<Uuid, u64>> {
        let rows = sqlx::query("SELECT install, seq FROM sync_cursors WHERE space = ?")
            .bind(space)
            .fetch_all(self.writer())
            .await?;
        Ok(rows.iter().map(|r| (r.get(0), u(r.get(1)))).collect())
    }

    pub(crate) async fn set_sync_cursor(&self, space: i64, install: Uuid, seq: u64) -> Result<()> {
        sqlx::query(
            "INSERT INTO sync_cursors (space, install, seq) VALUES (?, ?, ?)
             ON CONFLICT DO UPDATE SET seq = max(seq, excluded.seq)",
        )
        .bind(space)
        .bind(install)
        .bind(i(seq))
        .execute(self.writer())
        .await?;
        Ok(())
    }

    pub(crate) async fn upsert_device(&self, space: i64, d: &DeviceRow) -> Result<()> {
        sqlx::query(
            "INSERT INTO sync_devices (space, install, name, app_version, last_seen) VALUES (?, ?, ?, ?, ?)
             ON CONFLICT DO UPDATE SET name = excluded.name, app_version = excluded.app_version,
               last_seen = max(last_seen, excluded.last_seen)",
        )
        .bind(space)
        .bind(d.install)
        .bind(&d.name)
        .bind(&d.app_version)
        .bind(d.last_seen)
        .execute(self.writer())
        .await?;
        Ok(())
    }

    pub(crate) async fn sync_devices(&self, space: i64) -> Result<Vec<DeviceRow>> {
        let rows = sqlx::query(
            "SELECT install, name, app_version, last_seen FROM sync_devices WHERE space = ? ORDER BY name",
        )
        .bind(space)
        .fetch_all(self.writer())
        .await?;
        Ok(rows
            .iter()
            .map(|r| DeviceRow {
                install: r.get(0),
                name: r.get(1),
                app_version: r.get(2),
                last_seen: r.get(3),
            })
            .collect())
    }

    pub(crate) async fn remove_device(&self, space: i64, install: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM sync_devices WHERE space = ? AND install = ?")
            .bind(space)
            .bind(install)
            .execute(self.writer())
            .await?;
        sqlx::query("DELETE FROM sync_cursors WHERE space = ? AND install = ?")
            .bind(space)
            .bind(install)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    /// Whether `install` takes part in a space that `lib` is linked in
    /// (its sidecars there are already merged through the log).
    pub(crate) async fn syncs_library_with(&self, lib: LibraryId, install: &str) -> Result<bool> {
        let Ok(install) = Uuid::parse_str(install) else {
            return Ok(false);
        };
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM sync_links l JOIN sync_devices d ON d.space = l.space
             WHERE l.library_id = ? AND d.install = ? LIMIT 1",
        )
        .bind(lib)
        .bind(install)
        .fetch_optional(self.writer())
        .await?;
        Ok(found.is_some())
    }

    // ---- edit history ----

    pub(crate) async fn add_edit_history(
        &self,
        model: ModelId,
        key: &str,
        value: &serde_json::Value,
        device: &str,
        at: i64,
    ) -> Result<Uuid> {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO edit_history (id, model_id, key, value, device, at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(model)
        .bind(key)
        .bind(serde_json::to_string(value).unwrap_or_default())
        .bind(device)
        .bind(at)
        .execute(self.writer())
        .await?;
        Ok(id)
    }

    pub(crate) async fn edit_history(&self, model: ModelId) -> Result<Vec<EditEntry>> {
        let rows = sqlx::query(
            "SELECT id, model_id, key, value, device, at FROM edit_history
             WHERE model_id = ? AND dismissed = 0 ORDER BY at DESC",
        )
        .bind(model)
        .fetch_all(self.reader())
        .await?;
        Ok(rows.iter().filter_map(edit_entry).collect())
    }

    pub(crate) async fn edit_entry(&self, id: Uuid) -> Result<Option<EditEntry>> {
        let row = sqlx::query(
            "SELECT id, model_id, key, value, device, at FROM edit_history WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(self.writer())
        .await?;
        Ok(row.as_ref().and_then(edit_entry))
    }

    pub(crate) async fn dismiss_edit(&self, id: Uuid) -> Result<()> {
        sqlx::query("UPDATE edit_history SET dismissed = 1 WHERE id = ?")
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }

    // ---- what capture and matching read ----

    /// Present models of a library with their path and part fingerprints.
    pub(crate) async fn library_identities(
        &self,
        lib: LibraryId,
    ) -> Result<Vec<(ModelId, String, Vec<u64>)>> {
        let rows = sqlx::query(
            "SELECT m.id, m.rel_path, p.fp_hash FROM models m
             LEFT JOIN parts p ON p.model_id = m.id AND p.missing_since IS NULL AND p.fp_hash IS NOT NULL
             WHERE m.library_id = ? AND m.missing_since IS NULL ORDER BY m.id",
        )
        .bind(lib)
        .fetch_all(self.writer())
        .await?;
        let mut out: Vec<(ModelId, String, Vec<u64>)> = Vec::new();
        for r in &rows {
            let id: ModelId = r.get(0);
            let fp: Option<i64> = r.get(2);
            if out.last().is_none_or(|l| l.0 != id) {
                out.push((id, r.get(1), Vec::new()));
            }
            if let (Some(fp), Some(last)) = (fp, out.last_mut()) {
                last.2.push(fp as u64);
            }
        }
        for o in &mut out {
            o.2.sort_unstable();
            o.2.dedup();
        }
        Ok(out)
    }

    /// One model's library, path and part fingerprints.
    pub(crate) async fn model_identity(
        &self,
        id: ModelId,
    ) -> Result<Option<(LibraryId, String, Vec<u64>)>> {
        let Some(row) = sqlx::query("SELECT library_id, rel_path FROM models WHERE id = ?")
            .bind(id)
            .fetch_optional(self.writer())
            .await?
        else {
            return Ok(None);
        };
        let mut fps: Vec<u64> = sqlx::query_scalar::<_, i64>(
            "SELECT fp_hash FROM parts WHERE model_id = ? AND missing_since IS NULL AND fp_hash IS NOT NULL",
        )
        .bind(id)
        .fetch_all(self.writer())
        .await?
        .into_iter()
        .map(|h| h as u64)
        .collect();
        fps.sort_unstable();
        fps.dedup();
        Ok(Some((row.get(0), row.get(1), fps)))
    }

    /// Models of `lib` whose data, recipe or log changed after `since`.
    pub(crate) async fn models_changed_since(
        &self,
        lib: LibraryId,
        since: i64,
    ) -> Result<Vec<ModelId>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM models WHERE library_id = ?1 AND (data_updated_at > ?2
               OR id IN (SELECT model_id FROM recipes WHERE updated_at > ?2)
               OR id IN (SELECT model_id FROM attempts WHERE updated_at > ?2))",
        )
        .bind(lib)
        .bind(since)
        .fetch_all(self.writer())
        .await?)
    }

    /// Every model of `lib`.
    pub(crate) async fn library_model_ids(&self, lib: LibraryId) -> Result<Vec<ModelId>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM models WHERE library_id = ? ORDER BY id")
                .bind(lib)
                .fetch_all(self.writer())
                .await?,
        )
    }

    /// The library of model `id`.
    pub(crate) async fn model_library(&self, id: ModelId) -> Result<Option<LibraryId>> {
        Ok(
            sqlx::query_scalar("SELECT library_id FROM models WHERE id = ?")
                .bind(id)
                .fetch_optional(self.writer())
                .await?,
        )
    }

    // ---- catalogue numbers ----

    /// The model of `lib` that has number `n`.
    pub(crate) async fn number_holder(&self, lib: LibraryId, n: u32) -> Result<Option<ModelId>> {
        Ok(
            sqlx::query_scalar("SELECT id FROM models WHERE library_id = ? AND number = ?")
                .bind(lib)
                .bind(i64::from(n))
                .fetch_optional(self.writer())
                .await?,
        )
    }

    /// Gives model `id` number `n` (free in its library) and keeps the
    /// library's counter past it.
    pub(crate) async fn force_number(&self, id: ModelId, n: u32) -> Result<()> {
        let mut tx = self.writer().begin().await?;
        sqlx::query("UPDATE models SET number = ? WHERE id = ?")
            .bind(i64::from(n))
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE libraries SET next_number = max(next_number, ? + 1)
             WHERE id = (SELECT library_id FROM models WHERE id = ?)",
        )
        .bind(i64::from(n))
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Gives model `id` the next free number of its library.
    pub(crate) async fn renumber(&self, id: ModelId) -> Result<u32> {
        let mut tx = self.writer().begin().await?;
        let n: i64 = sqlx::query_scalar(
            "SELECT next_number FROM libraries WHERE id = (SELECT library_id FROM models WHERE id = ?)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("UPDATE models SET number = NULL WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        // Skip numbers another model already holds (taken from a sync).
        let mut n = n;
        loop {
            let taken: Option<i64> = sqlx::query_scalar(
                "SELECT 1 FROM models WHERE number = ? AND library_id = (SELECT library_id FROM models WHERE id = ?)",
            )
            .bind(n)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            if taken.is_none() {
                break;
            }
            n += 1;
        }
        sqlx::query("UPDATE models SET number = ? WHERE id = ?")
            .bind(n)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "UPDATE libraries SET next_number = ? + 1 WHERE id = (SELECT library_id FROM models WHERE id = ?)",
        )
        .bind(n)
        .bind(id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(u32::try_from(n).unwrap_or(u32::MAX))
    }

    /// Clears model `id`'s number (it moves to another model).
    pub(crate) async fn clear_number(&self, id: ModelId) -> Result<()> {
        sqlx::query("UPDATE models SET number = NULL WHERE id = ?")
            .bind(id)
            .execute(self.writer())
            .await?;
        Ok(())
    }
}

fn edit_entry(r: &sqlx::sqlite::SqliteRow) -> Option<EditEntry> {
    Some(EditEntry {
        id: r.get(0),
        model: r.get(1),
        key: r.get(2),
        value: serde_json::from_str(r.get::<&str, _>(3)).ok()?,
        device: r.get(4),
        at: r.get(5),
    })
}
