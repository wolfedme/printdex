//! What the thumbnail service needs from the database: where a part's or
//! a model's preview comes from, and part measurements.

use std::path::PathBuf;

use crate::db::Db;
use crate::error::Result;
use crate::ids::{ModelId, PartId};
use crate::walk::PrintableFormat;

/// A printable file to preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartSource {
    pub id: PartId,
    pub path: PathBuf,
    pub format: PrintableFormat,
    pub size: u64,
    pub mtime_ns: i64,
    pub fp_hash: Option<u64>,
    /// The model's material, which colours the render.
    pub material: printdex_render::Material,
}

/// An image file in a model folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageSource {
    pub path: PathBuf,
    pub rel_path: String,
    pub size: u64,
    pub mtime_ns: i64,
}

/// Everything a model's cover can come from, parts and images sorted by
/// path.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModelSources {
    pub parts: Vec<PartSource>,
    pub images: Vec<ImageSource>,
}

/// File extensions that count as images for model covers.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "webp"];

pub(crate) fn is_image(rel_path: &str) -> bool {
    rel_path
        .rsplit_once('.')
        .is_some_and(|(_, ext)| IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

fn format_from_db(s: &str) -> PrintableFormat {
    match s {
        "3mf" => PrintableFormat::ThreeMf,
        "obj" => PrintableFormat::Obj,
        _ => PrintableFormat::Stl,
    }
}

fn join(root: &str, rel: &str) -> PathBuf {
    let mut p = PathBuf::from(root);
    p.extend(rel.split('/'));
    p
}

/// The preview ramp for a stored material key (see `Material::key`).
pub fn render_material(key: Option<&str>) -> printdex_render::Material {
    match key.and_then(crate::model_data::Material::parse) {
        Some(crate::model_data::Material::Petg) => printdex_render::Material::Petg,
        Some(crate::model_data::Material::Pla) => printdex_render::Material::Pla,
        Some(crate::model_data::Material::Tpu) => printdex_render::Material::Tpu,
        _ => printdex_render::Material::Unknown,
    }
}

impl Db {
    /// Part `id` as a preview source; `None` when unknown or missing.
    pub async fn part_source(&self, id: PartId) -> Result<Option<PartSource>> {
        let row = sqlx::query!(
            r#"SELECT l.path as lib_path, p.rel_path, p.format, p.size, p.mtime_ns, p.fp_hash,
                      coalesce((SELECT r.material FROM recipes r WHERE r.model_id = p.model_id),
                               (SELECT a.material FROM attempts a WHERE a.model_id = p.model_id
                                  AND a.material IS NOT NULL ORDER BY a.printed_at DESC, a.id DESC LIMIT 1))
                        as "material?: String"
               FROM parts p JOIN libraries l ON l.id = p.library_id
               WHERE p.id = ? AND p.missing_since IS NULL"#,
            id
        )
        .fetch_optional(self.reader())
        .await?;
        Ok(row.map(|r| PartSource {
            id,
            path: join(&r.lib_path, &r.rel_path),
            format: format_from_db(&r.format),
            size: u64::try_from(r.size).unwrap_or(0),
            mtime_ns: r.mtime_ns,
            fp_hash: r.fp_hash.map(|h| h as u64),
            material: render_material(r.material.as_deref()),
        }))
    }

    /// Present parts and images of model `id`; `None` when unknown or
    /// missing.
    pub async fn model_sources(&self, id: ModelId) -> Result<Option<ModelSources>> {
        let Some(lib) = sqlx::query!(
            r#"SELECT l.path as lib_path,
                      coalesce((SELECT r.material FROM recipes r WHERE r.model_id = m.id),
                               (SELECT a.material FROM attempts a WHERE a.model_id = m.id
                                  AND a.material IS NOT NULL ORDER BY a.printed_at DESC, a.id DESC LIMIT 1))
                        as "material?: String"
               FROM models m JOIN libraries l ON l.id = m.library_id
               WHERE m.id = ? AND m.missing_since IS NULL"#,
            id
        )
        .fetch_optional(self.reader())
        .await?
        else {
            return Ok(None);
        };
        let parts = sqlx::query!(
            r#"SELECT id as "id!: PartId", rel_path, format, size, mtime_ns, fp_hash FROM parts
               WHERE model_id = ? AND missing_since IS NULL ORDER BY rel_path"#,
            id
        )
        .fetch_all(self.reader())
        .await?
        .into_iter()
        .map(|r| PartSource {
            id: r.id,
            path: join(&lib.lib_path, &r.rel_path),
            format: format_from_db(&r.format),
            size: u64::try_from(r.size).unwrap_or(0),
            mtime_ns: r.mtime_ns,
            fp_hash: r.fp_hash.map(|h| h as u64),
            material: render_material(lib.material.as_deref()),
        })
        .collect();
        let images = sqlx::query!(
            r#"SELECT rel_path, size, mtime_ns FROM assets
               WHERE model_id = ? AND missing_since IS NULL ORDER BY rel_path"#,
            id
        )
        .fetch_all(self.reader())
        .await?
        .into_iter()
        .filter(|r| is_image(&r.rel_path))
        .map(|r| ImageSource {
            path: join(&lib.lib_path, &r.rel_path),
            rel_path: r.rel_path,
            size: u64::try_from(r.size).unwrap_or(0),
            mtime_ns: r.mtime_ns,
        })
        .collect();
        Ok(Some(ModelSources { parts, images }))
    }

    /// Stores part `id`'s size in mm and triangle count, measured from the
    /// file with fingerprint hash `fp`.
    pub async fn set_measurements(
        &self,
        id: PartId,
        fp: Option<u64>,
        dims: [f32; 3],
        triangles: u64,
    ) -> Result<()> {
        let (x, y, z) = (f64::from(dims[0]), f64::from(dims[1]), f64::from(dims[2]));
        let tris = i64::try_from(triangles).unwrap_or(i64::MAX);
        let fp = fp.map(|h| h as i64);
        sqlx::query!(
            "UPDATE parts SET dim_x = ?, dim_y = ?, dim_z = ?, triangles = ?, measured_fp = ?
             WHERE id = ?",
            x,
            y,
            z,
            tris,
            fp,
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

    #[test]
    fn images_by_extension() {
        assert!(is_image("a/cover.JPG"));
        assert!(is_image("x.webp"));
        assert!(!is_image("model.stl"));
        assert!(!is_image("png"));
    }
}
