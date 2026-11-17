//! One model with its files, for the model view.

use crate::db::Db;
use crate::error::Result;
use crate::grouping::ModelKind;
use crate::ids::{AssetId, LibraryId, ModelId, PartId};
use crate::walk::PrintableFormat;

/// A printable file of a model.
#[derive(Clone, Debug, PartialEq)]
pub struct PartInfo {
    pub id: PartId,
    /// Relative to the library root, `/`-separated.
    pub rel_path: String,
    pub format: PrintableFormat,
    pub size: u64,
    /// Size along X, Y and Z in mm, once the file has been measured.
    pub dims: Option<[f32; 3]>,
    /// Triangle count, once the file has been measured.
    pub triangles: Option<u64>,
}

/// A non-printable file of a model (image, PDF, readme…).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetInfo {
    pub id: AssetId,
    pub rel_path: String,
    pub size: u64,
}

/// A model with its present files.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelDetail {
    pub id: ModelId,
    pub library_id: LibraryId,
    pub name: String,
    pub rel_path: String,
    pub kind: ModelKind,
    /// Catalogue number in its library (№ 0412).
    pub number: Option<u32>,
    /// When Printdex first saw the model, unix milliseconds.
    pub created_at: i64,
    pub parts: Vec<PartInfo>,
    pub assets: Vec<AssetInfo>,
}

impl ModelDetail {
    pub fn total_size(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum::<u64>()
            + self.assets.iter().map(|a| a.size).sum::<u64>()
    }
}

fn format_from_db(s: &str) -> PrintableFormat {
    match s {
        "3mf" => PrintableFormat::ThreeMf,
        "obj" => PrintableFormat::Obj,
        _ => PrintableFormat::Stl,
    }
}

fn size(v: i64) -> u64 {
    u64::try_from(v).unwrap_or(0)
}

impl Db {
    /// Model `id` with its present parts and assets sorted by path, or
    /// `None` when there is no such model or it is missing on disk.
    pub async fn model_detail(&self, id: ModelId) -> Result<Option<ModelDetail>> {
        let Some(m) = sqlx::query!(
            r#"SELECT id as "id!: ModelId", library_id as "library_id!: LibraryId",
                      name, rel_path, kind, created_at, number
               FROM models WHERE id = ? AND missing_since IS NULL"#,
            id
        )
        .fetch_optional(self.reader())
        .await?
        else {
            return Ok(None);
        };
        let parts = sqlx::query!(
            r#"SELECT id as "id!: PartId", rel_path, format, size,
                      CASE WHEN measured_fp IS fp_hash THEN dim_x END as "dim_x?: f64",
                      CASE WHEN measured_fp IS fp_hash THEN dim_y END as "dim_y?: f64",
                      CASE WHEN measured_fp IS fp_hash THEN dim_z END as "dim_z?: f64",
                      CASE WHEN measured_fp IS fp_hash THEN triangles END as "triangles?: i64"
               FROM parts
               WHERE model_id = ? AND missing_since IS NULL ORDER BY rel_path"#,
            id
        )
        .fetch_all(self.reader())
        .await?
        .into_iter()
        .map(|r| PartInfo {
            id: r.id,
            rel_path: r.rel_path,
            format: format_from_db(&r.format),
            size: size(r.size),
            dims: match (r.dim_x, r.dim_y, r.dim_z) {
                (Some(x), Some(y), Some(z)) => Some([x as f32, y as f32, z as f32]),
                _ => None,
            },
            triangles: r.triangles.and_then(|t| u64::try_from(t).ok()),
        })
        .collect();
        let assets = sqlx::query!(
            r#"SELECT id as "id!: AssetId", rel_path, size FROM assets
               WHERE model_id = ? AND missing_since IS NULL ORDER BY rel_path"#,
            id
        )
        .fetch_all(self.reader())
        .await?
        .into_iter()
        .map(|r| AssetInfo {
            id: r.id,
            rel_path: r.rel_path,
            size: size(r.size),
        })
        .collect();
        Ok(Some(ModelDetail {
            id: m.id,
            library_id: m.library_id,
            name: m.name,
            rel_path: m.rel_path,
            kind: ModelKind::from_db(&m.kind).unwrap_or(ModelKind::Folder),
            number: m.number.and_then(|n| u32::try_from(n).ok()),
            created_at: m.created_at,
            parts,
            assets,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Library, LibraryKind};
    use std::path::PathBuf;

    #[tokio::test]
    async fn model_detail_lists_parts_and_assets() {
        let d = tempfile::tempdir().unwrap();
        let db = Db::open(&d.path().join("t.db")).await.unwrap();
        let lib = Library {
            id: LibraryId::new(),
            path: PathBuf::from("/lib"),
            display_name: "lib".into(),
            kind: LibraryKind::Local,
            sidecar_enabled: true,
            last_full_scan_at: None,
        };
        db.insert_library(&lib).await.unwrap();
        let m = ModelId::new();
        sqlx::query(
            "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
             VALUES (?, ?, 'Bin', 'Bin', 'folder', 42, 42)",
        )
        .bind(m)
        .bind(lib.id)
        .execute(db.writer())
        .await
        .unwrap();
        for (path, format, size, missing) in [
            ("Bin/z.3mf", "3mf", 30_i64, None),
            ("Bin/a.stl", "stl", 10, None),
            ("Bin/gone.stl", "stl", 5, Some(9_i64)),
        ] {
            sqlx::query(
                "INSERT INTO parts (id, model_id, library_id, rel_path, format, size, mtime_ns,
                                    missing_since, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, 1, ?, 1, 1)",
            )
            .bind(PartId::new())
            .bind(m)
            .bind(lib.id)
            .bind(path)
            .bind(format)
            .bind(size)
            .bind(missing)
            .execute(db.writer())
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO assets (id, model_id, library_id, rel_path, size, mtime_ns, created_at, updated_at)
             VALUES (?, ?, ?, 'Bin/photo.jpg', 7, 1, 1, 1)",
        )
        .bind(AssetId::new())
        .bind(m)
        .bind(lib.id)
        .execute(db.writer())
        .await
        .unwrap();

        let detail = db.model_detail(m).await.unwrap().unwrap();
        assert_eq!(detail.name, "Bin");
        assert_eq!(detail.created_at, 42);
        let parts: Vec<_> = detail
            .parts
            .iter()
            .map(|p| (p.rel_path.as_str(), p.format))
            .collect();
        assert_eq!(
            parts,
            [
                ("Bin/a.stl", PrintableFormat::Stl),
                ("Bin/z.3mf", PrintableFormat::ThreeMf)
            ]
        );
        assert_eq!(detail.assets.len(), 1);
        assert_eq!(detail.total_size(), 47);
        assert_eq!(db.model_detail(ModelId::new()).await.unwrap(), None);
    }
}
