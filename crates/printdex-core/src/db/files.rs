//! Part and asset rows, and applying a scan's results.

use std::collections::{HashMap, HashSet};

use crate::db::Db;
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::identity::{Arrival, Candidate, Key, match_moves};
use crate::ids::{AssetId, LibraryId, ModelId, PartId};
use crate::model_data::Material;
use crate::scan::{ApplyStats, Moved, ScanWrite};
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
    /// Fingerprint hash the 3MF material was read at.
    pub material_fp: Option<u64>,
    pub is_part: bool,
    /// Seen by the last scan that reached it (`missing_since IS NULL`).
    pub present: bool,
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
        rel_path: &'a str,
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
        /// The 3MF material read this scan, if it was read.
        material: Option<Option<&'a str>>,
    },
    UpdatePart {
        id: PartId,
        model: ModelId,
        rel_path: &'a str,
        format: &'static str,
        size: i64,
        mtime_ns: i64,
        fp_hash: Option<i64>,
        material: Option<Option<&'a str>>,
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
/// `rel_path` key ranges that cover `scope` (everything when `None`).
/// A range also catches siblings like `Busts-old` for scope `Busts`, so
/// rows still go through [`in_scope`].
fn scope_ranges(scope: Option<&[String]>) -> Vec<(String, String)> {
    match scope {
        // Above every valid UTF-8 path.
        None => vec![(String::new(), "\u{10FFFF}".to_owned())],
        // '0' sorts right after '/'.
        Some(s) => s.iter().map(|p| (p.clone(), format!("{p}0"))).collect(),
    }
}

/// Whether `path` is one of the `scope` paths or inside one.
pub(crate) fn in_scope(path: &str, scope: Option<&[String]>) -> bool {
    scope.is_none_or(|s| {
        s.iter().any(|p| {
            path == p
                || (path.starts_with(p.as_str()) && path.as_bytes().get(p.len()) == Some(&b'/'))
        })
    })
}

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
    pub async fn known_files(
        &self,
        lib: LibraryId,
        scope: Option<&[String]>,
    ) -> Result<Vec<KnownFile>> {
        let mut out = Vec::new();
        for (lo, hi) in scope_ranges(scope) {
            let parts = sqlx::query!(
                r#"SELECT rel_path, size, mtime_ns, fp_hash, material_fp, missing_since FROM parts
                   WHERE library_id = ? AND rel_path >= ? AND rel_path < ?"#,
                lib,
                lo,
                hi
            )
            .fetch_all(self.writer())
            .await?;
            let assets = sqlx::query!(
                r#"SELECT rel_path, size, mtime_ns, missing_since FROM assets
                   WHERE library_id = ? AND rel_path >= ? AND rel_path < ?"#,
                lib,
                lo,
                hi
            )
            .fetch_all(self.writer())
            .await?;
            out.extend(parts.into_iter().map(|r| KnownFile {
                rel_path: r.rel_path,
                size: r.size as u64,
                mtime_ns: r.mtime_ns,
                fp_hash: r.fp_hash.map(|h| h as u64),
                material_fp: r.material_fp.map(|h| h as u64),
                is_part: true,
                present: r.missing_since.is_none(),
            }));
            out.extend(assets.into_iter().map(|r| KnownFile {
                rel_path: r.rel_path,
                size: r.size as u64,
                mtime_ns: r.mtime_ns,
                fp_hash: None,
                material_fp: None,
                is_part: false,
                present: r.missing_since.is_none(),
            }));
        }
        out.retain(|k| in_scope(&k.rel_path, scope));
        Ok(out)
    }

    /// Writes a scan's models, parts and assets. Rows are matched by
    /// relative path and keep their ids; rows the scan did not see are marked
    /// missing, never deleted. Unchanged rows are not written at all.
    pub async fn apply_scan(&self, lib: LibraryId, write: ScanWrite) -> Result<ApplyStats> {
        let scope = write.scope.as_deref();
        let (mut models, mut parts, mut assets) = (Vec::new(), Vec::new(), Vec::new());
        for (lo, hi) in scope_ranges(scope) {
            models.extend(
                sqlx::query_as!(
                    ModelRow,
                    r#"SELECT id as "id!: ModelId", rel_path, name, kind, missing_since
                       FROM models WHERE library_id = ? AND rel_path >= ? AND rel_path < ?"#,
                    lib,
                    lo,
                    hi
                )
                .fetch_all(self.writer())
                .await?,
            );
            parts.extend(
                sqlx::query_as!(
                    PartRow,
                    r#"SELECT id as "id!: PartId", model_id as "model_id: ModelId", rel_path, format,
                              size, mtime_ns, fp_hash, missing_since
                       FROM parts WHERE library_id = ? AND rel_path >= ? AND rel_path < ?"#,
                    lib,
                    lo,
                    hi
                )
                .fetch_all(self.writer())
                .await?,
            );
            assets.extend(
                sqlx::query_as!(
                    AssetRow,
                    r#"SELECT id as "id!: AssetId", model_id as "model_id: ModelId", rel_path,
                              size, mtime_ns, missing_since
                       FROM assets WHERE library_id = ? AND rel_path >= ? AND rel_path < ?"#,
                    lib,
                    lo,
                    hi
                )
                .fetch_all(self.writer())
                .await?,
            );
        }
        models.retain(|m| in_scope(&m.rel_path, scope));
        parts.retain(|p| in_scope(&p.rel_path, scope));
        assets.retain(|a| in_scope(&a.rel_path, scope));

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

        // Models that moved: groups at a path with no model row, matched by
        // part fingerprints to models not found at their old path.
        let path_matched: HashSet<ModelId> = write
            .groups
            .iter()
            .filter_map(|g| models_by_path.get(g.rel_path.as_str()).map(|m| m.id))
            .collect();
        let mut parts_of: HashMap<ModelId, Vec<&PartRow>> = HashMap::new();
        for p in &parts {
            parts_of.entry(p.model_id).or_default().push(p);
        }
        let key_of = |p: &PartRow| p.fp_hash.map(|h| (p.size as u64, h as u64));
        let cand_rows: Vec<&ModelRow> = models
            .iter()
            .filter(|m| !path_matched.contains(&m.id))
            .filter(|m| {
                parts_of
                    .get(&m.id)
                    .is_some_and(|ps| ps.iter().any(|p| key_of(p).is_some()))
            })
            .collect();
        let candidates: Vec<Candidate> = cand_rows
            .iter()
            .filter_map(|m| {
                Some(Candidate {
                    kind: ModelKind::from_db(&m.kind)?,
                    keys: parts_of[&m.id].iter().filter_map(|p| key_of(p)).collect(),
                })
            })
            .collect();
        // `from_db` never fails for rows we wrote; keep indices aligned anyway.
        let cand_rows: Vec<&ModelRow> = cand_rows
            .into_iter()
            .filter(|m| ModelKind::from_db(&m.kind).is_some())
            .collect();
        let file_key = |i: usize| -> Option<Key> {
            let f = &write.files[i];
            write
                .fingerprints
                .get(&i)
                .map(|fp| (fp.size, fp.hash))
                .or_else(|| {
                    parts_by_path
                        .get(f.rel_path.as_str())
                        .and_then(|p| key_of(p))
                })
        };
        let arriving: Vec<usize> = (0..write.groups.len())
            .filter(|&g| !models_by_path.contains_key(write.groups[g].rel_path.as_str()))
            .collect();
        let arrivals: Vec<Arrival> = arriving
            .iter()
            .map(|&g| {
                let group = &write.groups[g];
                Arrival {
                    kind: group.kind,
                    keys: group
                        .parts
                        .iter()
                        .filter(|&&i| matches!(write.files[i].kind, FileKind::Printable(_)))
                        .map(|&i| file_key(i))
                        .collect(),
                }
            })
            .collect();
        let moved_to: HashMap<usize, &ModelRow> = match_moves(&arrivals, &candidates)
            .into_iter()
            .map(|(a, c)| (arriving[a], cand_rows[c]))
            .collect();
        // Old parts of moved models, by key, to carry their ids over.
        let mut reusable: HashMap<(ModelId, Key), Vec<&PartRow>> = HashMap::new();
        for row in moved_to.values() {
            for p in parts_of.get(&row.id).into_iter().flatten() {
                if let Some(k) = key_of(p) {
                    reusable.entry((row.id, k)).or_default().push(p);
                }
            }
        }

        for (g, group) in write.groups.iter().enumerate() {
            let kind = group.kind.as_str();
            let moved = moved_to.get(&g);
            let model_id = match models_by_path.get(group.rel_path.as_str()).or(moved) {
                Some(row) if row.rel_path != group.rel_path => {
                    // Moved or renamed: same model, new place.
                    stats.moved.push(Moved {
                        model: row.id,
                        kind: group.kind,
                        from: row.rel_path.clone(),
                        to: group.rel_path.clone(),
                    });
                    ops.push(Op::UpdateModel {
                        id: row.id,
                        rel_path: &group.rel_path,
                        name: &group.name,
                        kind,
                    });
                    seen_models.insert(row.id);
                    row.id
                }
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
                            rel_path: &group.rel_path,
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
                let material = write.materials.get(&i).map(|m| m.as_deref());
                match parts_by_path.get(file.rel_path.as_str()) {
                    Some(row) => {
                        seen_parts.insert(row.id);
                        let new_hash = fp_hash.or(row.fp_hash);
                        let changed = row.model_id != model_id
                            || row.format != format
                            || row.size != size
                            || row.mtime_ns != file.mtime_ns
                            || row.fp_hash != new_hash
                            || material.is_some()
                            || row.missing_since.is_some();
                        if changed {
                            ops.push(Op::UpdatePart {
                                id: row.id,
                                model: model_id,
                                rel_path: &file.rel_path,
                                format,
                                size,
                                mtime_ns: file.mtime_ns,
                                fp_hash: new_hash,
                                material,
                            });
                        }
                    }
                    None => {
                        // A moved model's part keeps its id (and thumbnail).
                        let old = file_key(i)
                            .and_then(|k| reusable.get_mut(&(model_id, k)).and_then(|v| v.pop()));
                        match old {
                            Some(row) => {
                                seen_parts.insert(row.id);
                                ops.push(Op::UpdatePart {
                                    id: row.id,
                                    model: model_id,
                                    rel_path: &file.rel_path,
                                    format,
                                    size,
                                    mtime_ns: file.mtime_ns,
                                    fp_hash: fp_hash.or(row.fp_hash),
                                    material,
                                });
                            }
                            None => ops.push(Op::InsertPart {
                                id: PartId::new(),
                                model: model_id,
                                rel_path: &file.rel_path,
                                format,
                                size,
                                mtime_ns: file.mtime_ns,
                                fp_hash,
                                material,
                            }),
                        }
                    }
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
                    Op::UpdateModel {
                        id,
                        rel_path,
                        name,
                        kind,
                    } => {
                        sqlx::query!(
                            "UPDATE models SET rel_path = ?, name = ?, kind = ?, missing_since = NULL,
                                               updated_at = ?
                             WHERE id = ?",
                            rel_path, name, kind, now, id
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
                        material,
                    } => {
                        let (read, file_material) = (material.is_some(), material.flatten());
                        let material_fp = fp_hash.filter(|_| read);
                        sqlx::query!(
                            "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns,
                                                fp_hash, file_material, material_fp, created_at, updated_at)
                             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                            id, model, lib, rel_path, format, size, mtime_ns, fp_hash, file_material,
                            material_fp, now, now
                        )
                        .execute(&mut *tx)
                        .await?;
                    }
                    Op::UpdatePart {
                        id,
                        model,
                        rel_path,
                        format,
                        size,
                        mtime_ns,
                        fp_hash,
                        material,
                    } => {
                        // Unread this scan: keep what was read before.
                        let (read, file_material) = (material.is_some(), material.flatten());
                        sqlx::query!(
                            "UPDATE parts SET model_id = ?1, rel_path = ?2, format = ?3, size = ?4,
                                              mtime_ns = ?5, fp_hash = ?6, missing_since = NULL,
                                              updated_at = ?7,
                                              file_material = CASE WHEN ?8 THEN ?9 ELSE file_material END,
                                              material_fp = CASE WHEN ?8 THEN ?6 ELSE material_fp END,
                                              path_material = CASE WHEN rel_path IS ?2 THEN path_material END
                             WHERE id = ?10",
                            model, rel_path, format, size, mtime_ns, fp_hash, now, read,
                            file_material, id
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
        self.fill_path_materials().await?;
        Ok(stats)
    }

    /// Looks up the material word in the path of every part that has not
    /// been looked at since it was added or moved.
    pub(crate) async fn fill_path_materials(&self) -> Result<()> {
        loop {
            let rows = sqlx::query!(
                r#"SELECT id as "id!: PartId", rel_path FROM parts
                   WHERE path_material IS NULL LIMIT 5000"#
            )
            .fetch_all(self.writer())
            .await?;
            if rows.is_empty() {
                return Ok(());
            }
            let mut tx = self.writer().begin().await?;
            for r in rows {
                let key = Material::infer_from_path(&r.rel_path)
                    .map(|m| m.key())
                    .unwrap_or_default();
                sqlx::query!("UPDATE parts SET path_material = ? WHERE id = ?", key, r.id)
                    .execute(&mut *tx)
                    .await?;
            }
            tx.commit().await?;
        }
    }
}
