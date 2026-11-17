//! Part and asset rows, and applying a scan's results.

use std::collections::{HashMap, HashSet};

use crate::db::Db;
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::ids::{AssetId, LibraryId, ModelId, PartId};
use crate::scan::{ApplyStats, ScanWrite};
use crate::time::now_ms;
use crate::walk::FileKind;

/// Rows per write transaction.
const BATCH_ROWS: usize = 5_000;

/// What the database knows about a file from earlier scans.
#[derive(Clone, Debug)]
pub struct KnownFile {
    pub rel_path: String,
    pub size: u64,
    pub mtime_ns: i64,
    pub fp_hash: Option<u64>,
    pub is_part: bool,
}

struct ModelRow {
    id: ModelId,
    rel_path: String,
    name: String,
    kind: String,
    missing_since: Option<i64>,
}

struct PartRow {
    id: PartId,
    model_id: ModelId,
    rel_path: String,
    format: String,
    size: i64,
    mtime_ns: i64,
    fp_hash: Option<i64>,
    missing_since: Option<i64>,
}

struct AssetRow {
    id: AssetId,
    model_id: ModelId,
    rel_path: String,
    size: i64,
    mtime_ns: i64,
    missing_since: Option<i64>,
}

/// One planned write.
enum Op<'a> {
    InsertModel {
        id: ModelId,
        rel_path: &'a str,
        name: &'a str,
        kind: &'static str,
    },
    UpdateModel {
        id: ModelId,
        name: &'a str,
        kind: &'static str,
    },
    InsertPart {
        id: PartId,
        model: ModelId,
        rel_path: &'a str,
        format: &'static str,
        size: i64,
        mtime_ns: i64,
        fp_hash: Option<i64>,
    },
    UpdatePart {
        id: PartId,
        model: ModelId,
        format: &'static str,
        size: i64,
        mtime_ns: i64,
        fp_hash: Option<i64>,
    },
    InsertAsset {
        id: AssetId,
        model: ModelId,
        rel_path: &'a str,
        size: i64,
        mtime_ns: i64,
    },
    UpdateAsset {
        id: AssetId,
        model: ModelId,
        size: i64,
        mtime_ns: i64,
    },
    MissingModel(ModelId),
    MissingPart(PartId),
    MissingAsset(AssetId),
}

/// Whether `a` and `b` are the same path or one lies inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    let inside = |inner: &str, outer: &str| {
        inner.len() > outer.len()
            && inner.starts_with(outer)
            && inner.as_bytes()[outer.len()] == b'/'
    };
    a == b || inside(a, b) || inside(b, a)
}

impl Db {
    /// Every part and asset of `lib` recorded so far, missing or not.
    pub async fn known_files(&self, lib: LibraryId) -> Result<Vec<KnownFile>> {
        let parts = sqlx::query!(
            r#"SELECT rel_path, size, mtime_ns, fp_hash FROM parts WHERE library_id = ?"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;
        let assets = sqlx::query!(
            r#"SELECT rel_path, size, mtime_ns FROM assets WHERE library_id = ?"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;
        let mut out = Vec::with_capacity(parts.len() + assets.len());
        out.extend(parts.into_iter().map(|r| KnownFile {
            rel_path: r.rel_path,
            size: r.size as u64,
            mtime_ns: r.mtime_ns,
            fp_hash: r.fp_hash.map(|h| h as u64),
            is_part: true,
        }));
        out.extend(assets.into_iter().map(|r| KnownFile {
            rel_path: r.rel_path,
            size: r.size as u64,
            mtime_ns: r.mtime_ns,
            fp_hash: None,
            is_part: false,
        }));
        Ok(out)
    }

    /// Writes a scan's models, parts and assets. Rows are matched by
    /// relative path and keep their ids; rows the scan did not see are marked
    /// missing, never deleted. Unchanged rows are not written at all.
    pub async fn apply_scan(&self, lib: LibraryId, write: ScanWrite) -> Result<ApplyStats> {
        let models = sqlx::query_as!(
            ModelRow,
            r#"SELECT id as "id!: ModelId", rel_path, name, kind, missing_since
               FROM models WHERE library_id = ?"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;
        let parts = sqlx::query_as!(
            PartRow,
            r#"SELECT id as "id!: PartId", model_id as "model_id: ModelId", rel_path, format,
                      size, mtime_ns, fp_hash, missing_since
               FROM parts WHERE library_id = ?"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;
        let assets = sqlx::query_as!(
            AssetRow,
            r#"SELECT id as "id!: AssetId", model_id as "model_id: ModelId", rel_path,
                      size, mtime_ns, missing_since
               FROM assets WHERE library_id = ?"#,
            lib
        )
        .fetch_all(self.writer())
        .await?;

        let models_by_path: HashMap<&str, &ModelRow> =
            models.iter().map(|m| (m.rel_path.as_str(), m)).collect();
        let parts_by_path: HashMap<&str, &PartRow> =
            parts.iter().map(|p| (p.rel_path.as_str(), p)).collect();
        let assets_by_path: HashMap<&str, &AssetRow> =
            assets.iter().map(|a| (a.rel_path.as_str(), a)).collect();

        let mut stats = ApplyStats::default();
        let mut ops = Vec::new();
        let mut seen_models = HashSet::new();
        let mut seen_parts = HashSet::new();
        let mut seen_assets = HashSet::new();

        for group in &write.groups {
            let kind = group.kind.as_str();
            let model_id = match models_by_path.get(group.rel_path.as_str()) {
                Some(row) => {
                    if row.missing_since.is_some() {
                        stats.models_returned += 1;
                    }
                    let changed = row.name != group.name
                        || ModelKind::from_db(&row.kind) != Some(group.kind)
                        || row.missing_since.is_some();
                    if changed {
                        ops.push(Op::UpdateModel {
                            id: row.id,
                            name: &group.name,
                            kind,
                        });
                    }
                    seen_models.insert(row.id);
                    row.id
                }
                None => {
                    let id = ModelId::new();
                    stats.models_added += 1;
                    ops.push(Op::InsertModel {
                        id,
                        rel_path: &group.rel_path,
                        name: &group.name,
                        kind,
                    });
                    id
                }
            };

            for &i in &group.parts {
                let file = &write.files[i];
                let FileKind::Printable(format) = file.kind else {
                    continue;
                };
                let format = format.as_str();
                let size = file.size as i64;
                let fp_hash = write.fingerprints.get(&i).map(|f| f.hash as i64);
                match parts_by_path.get(file.rel_path.as_str()) {
                    Some(row) => {
                        seen_parts.insert(row.id);
                        let new_hash = fp_hash.or(row.fp_hash);
                        let changed = row.model_id != model_id
                            || row.format != format
                            || row.size != size
                            || row.mtime_ns != file.mtime_ns
                            || row.fp_hash != new_hash
                            || row.missing_since.is_some();
                        if changed {
                            ops.push(Op::UpdatePart {
                                id: row.id,
                                model: model_id,
                                format,
                                size,
                                mtime_ns: file.mtime_ns,
                                fp_hash: new_hash,
                            });
                        }
                    }
                    None => ops.push(Op::InsertPart {
                        id: PartId::new(),
                        model: model_id,
                        rel_path: &file.rel_path,
                        format,
                        size,
                        mtime_ns: file.mtime_ns,
                        fp_hash,
                    }),
                }
            }

            for &i in &group.assets {
                let file = &write.files[i];
                let size = file.size as i64;
                match assets_by_path.get(file.rel_path.as_str()) {
                    Some(row) => {
                        seen_assets.insert(row.id);
                        let changed = row.model_id != model_id
                            || row.size != size
                            || row.mtime_ns != file.mtime_ns
                            || row.missing_since.is_some();
                        if changed {
                            ops.push(Op::UpdateAsset {
                                id: row.id,
                                model: model_id,
                                size,
                                mtime_ns: file.mtime_ns,
                            });
                        }
                    }
                    None => ops.push(Op::InsertAsset {
                        id: AssetId::new(),
                        model: model_id,
                        rel_path: &file.rel_path,
                        size,
                        mtime_ns: file.mtime_ns,
                    }),
                }
            }
        }

        let kept = |path: &str| write.keep.iter().any(|k| overlaps(path, k));
        for m in &models {
            if m.missing_since.is_none() && !seen_models.contains(&m.id) && !kept(&m.rel_path) {
                stats.models_missing += 1;
                ops.push(Op::MissingModel(m.id));
            }
        }
        for p in &parts {
            if p.missing_since.is_none() && !seen_parts.contains(&p.id) && !kept(&p.rel_path) {
                ops.push(Op::MissingPart(p.id));
            }
        }
        for a in &assets {
            if a.missing_since.is_none() && !seen_assets.contains(&a.id) && !kept(&a.rel_path) {
                ops.push(Op::MissingAsset(a.id));
            }
        }

        let now = now_ms();
        for batch in ops.chunks(BATCH_ROWS) {
            let mut tx = self.writer().begin().await?;
            for op in batch {
                match *op {
                    Op::InsertModel {
                        id,
                        rel_path,
                        name,
                        kind,
                    } => {
                        sqlx::query!(
                            "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
                             VALUES (?, ?, ?, ?, ?, ?, ?)",
                            id, lib, rel_path, name, kind, now, now
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::UpdateModel { id, name, kind } => {
                        sqlx::query!(
                            "UPDATE models SET name = ?, kind = ?, missing_since = NULL, updated_at = ?
                             WHERE id = ?",
                            name, kind, now, id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::InsertPart {
                        id,
                        model,
                        rel_path,
                        format,
                        size,
                        mtime_ns,
                        fp_hash,
                    } => {
                        sqlx::query!(
                            "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns,
                                                fp_hash, created_at, updated_at)
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                            id, model, lib, rel_path, format, size, mtime_ns, fp_hash, now, now
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::UpdatePart {
                        id,
                        model,
                        format,
                        size,
                        mtime_ns,
                        fp_hash,
                    } => {
                        sqlx::query!(
                            "UPDATE parts SET model_id = ?, format = ?, size = ?, mtime_ns = ?, fp_hash = ?,
                                              missing_since = NULL, updated_at = ?
                             WHERE id = ?",
                            model, format, size, mtime_ns, fp_hash, now, id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::InsertAsset {
                        id,
                        model,
                        rel_path,
                        size,
                        mtime_ns,
                    } => {
                        sqlx::query!(
                            "INSERT INTO assets (id, model_id, library_id, rel_path, size, mtime_ns,
                                                 created_at, updated_at)
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                            id, model, lib, rel_path, size, mtime_ns, now, now
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::UpdateAsset {
                        id,
                        model,
                        size,
                        mtime_ns,
                    } => {
                        sqlx::query!(
                            "UPDATE assets SET model_id = ?, size = ?, mtime_ns = ?, missing_since = NULL,
                                               updated_at = ?
                             WHERE id = ?",
                            model, size, mtime_ns, now, id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::MissingModel(id) => {
                        sqlx::query!(
                            "UPDATE models SET missing_since = ?, updated_at = ? WHERE id = ?",
                            now,
                            now,
                            id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::MissingPart(id) => {
                        sqlx::query!(
                            "UPDATE parts SET missing_since = ?, updated_at = ? WHERE id = ?",
                            now,
                            now,
                            id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::MissingAsset(id) => {
                        sqlx::query!(
                            "UPDATE assets SET missing_since = ?, updated_at = ? WHERE id = ?",
                            now,
                            now,
                            id
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            }
            tx.commit().await?;
        }
        Ok(stats)
    }
}
