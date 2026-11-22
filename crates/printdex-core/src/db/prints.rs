//! Recipes, print attempts and their photos.

use std::collections::BTreeSet;
use std::path::Path;

use sqlx::SqliteConnection;

use crate::db::Db;
use crate::db::organize::{ensure_model, refresh_fts};
use crate::error::{CoreError, Result};
use crate::ids::{AttemptId, ModelId, PhotoId};
use crate::model_data::{
    Attempt, AttemptResult, DURATION_MIN, NewAttempt, PartOverride, PhotoRef, PrintSettings,
    Recipe, RecipeNote, TEXT_MAX_BYTES,
};
use crate::time::now_ms;

/// Most photos per attempt.
pub const MAX_PHOTOS: usize = 8;

fn json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".into())
}

/// Parses a JSON column, falling back to the default for anything broken.
fn parse<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

fn material_key(s: &PrintSettings) -> Option<String> {
    s.material.as_ref().map(crate::model_data::Material::key)
}

fn check_attempt(settings: &PrintSettings, duration: Option<u32>, notes: &str) -> Result<()> {
    settings.validate()?;
    if let Some(d) = duration {
        DURATION_MIN.check(f64::from(d))?;
    }
    if notes.len() > TEXT_MAX_BYTES {
        return Err(CoreError::TextTooLong);
    }
    Ok(())
}

/// The recipe of model `id`, if it has one.
pub(crate) async fn load_recipe(
    conn: &mut SqliteConnection,
    id: ModelId,
) -> Result<Option<Recipe>> {
    let row = sqlx::query!(
        r#"SELECT settings, required, notes, parts, from_attempt as "from_attempt: AttemptId",
                  updated_at FROM recipes WHERE model_id = ?"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|r| Recipe {
        settings: parse(&r.settings),
        required: parse::<BTreeSet<String>>(&r.required),
        notes: parse::<Vec<RecipeNote>>(&r.notes),
        parts: parse::<Vec<PartOverride>>(&r.parts),
        from_attempt: r.from_attempt,
        updated_at: r.updated_at,
    }))
}

/// Recipe words for the full-text index ("" without a recipe).
pub(crate) async fn recipe_search_text(conn: &mut SqliteConnection, id: ModelId) -> Result<String> {
    Ok(load_recipe(conn, id)
        .await?
        .map(|r| r.search_text())
        .unwrap_or_default())
}

/// Attempts of model `id`, newest first, numbered from the oldest.
pub(crate) async fn load_attempts(
    conn: &mut SqliteConnection,
    id: ModelId,
    photo_dir: &Path,
) -> Result<Vec<Attempt>> {
    let rows = sqlx::query!(
        r#"SELECT id as "id!: AttemptId", printed_at, result, settings, duration_min, notes, parts,
                  updated_at
           FROM attempts WHERE model_id = ? ORDER BY printed_at, id"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let photos = sqlx::query!(
        r#"SELECT p.id as "id!: PhotoId", p.attempt_id as "attempt!: AttemptId", p.file_name
           FROM attempt_photos p JOIN attempts a ON a.id = p.attempt_id
           WHERE a.model_id = ? ORDER BY p.id"#,
        id
    )
    .fetch_all(&mut *conn)
    .await?;
    let mut out: Vec<Attempt> = rows
        .into_iter()
        .enumerate()
        .map(|(i, r)| Attempt {
            id: r.id,
            printed_at: r.printed_at,
            result: AttemptResult::from_db(&r.result).unwrap_or(AttemptResult::Failed),
            settings: parse(&r.settings),
            duration_min: r.duration_min.and_then(|d| u32::try_from(d).ok()),
            notes: r.notes,
            parts: parse(&r.parts),
            photos: photos
                .iter()
                .filter(|p| p.attempt == r.id)
                .map(|p| PhotoRef {
                    id: p.id,
                    path: photo_dir.join(r.id.to_string()).join(&p.file_name),
                })
                .collect(),
            number: u32::try_from(i + 1).unwrap_or(u32::MAX),
            updated_at: r.updated_at,
        })
        .collect();
    out.reverse();
    Ok(out)
}

impl Db {
    /// Sets or removes the recipe of model `id`.
    pub async fn set_recipe(&self, id: ModelId, recipe: Option<&Recipe>) -> Result<()> {
        if let Some(r) = recipe {
            r.validate()?;
        }
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        match recipe {
            Some(r) => write_recipe(&mut tx, id, r, now_ms()).await?,
            None => {
                sqlx::query!("DELETE FROM recipes WHERE model_id = ?", id)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        refresh_fts(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Replaces the recipe of model `id` with an imported one, keeping its
    /// `updated_at`. Invalid recipes are refused.
    pub async fn import_recipe(&self, id: ModelId, recipe: &Recipe) -> Result<()> {
        recipe.validate()?;
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        write_recipe(&mut tx, id, recipe, recipe.updated_at).await?;
        refresh_fts(&mut tx, id).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records a print of model `id`.
    pub async fn log_attempt(&self, id: ModelId, a: &NewAttempt) -> Result<AttemptId> {
        check_attempt(&a.settings, a.duration_min, &a.notes)?;
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, id).await?;
        let attempt = AttemptId::new();
        let now = now_ms();
        write_attempt(
            &mut tx,
            id,
            attempt,
            &AttemptFields {
                printed_at: a.printed_at,
                result: a.result,
                settings: &a.settings,
                duration_min: a.duration_min,
                notes: &a.notes,
                parts: &a.parts,
            },
            now,
        )
        .await?;
        tx.commit().await?;
        Ok(attempt)
    }

    /// Saves changes to an existing attempt. Returns its model.
    pub async fn update_attempt(&self, a: &Attempt) -> Result<ModelId> {
        check_attempt(&a.settings, a.duration_min, &a.notes)?;
        let mut tx = self.writer().begin().await?;
        let model = attempt_model(&mut tx, a.id).await?;
        write_attempt(
            &mut tx,
            model,
            a.id,
            &AttemptFields {
                printed_at: a.printed_at,
                result: a.result,
                settings: &a.settings,
                duration_min: a.duration_min,
                notes: &a.notes,
                parts: &a.parts,
            },
            now_ms(),
        )
        .await?;
        tx.commit().await?;
        Ok(model)
    }

    /// Writes an imported attempt when it is new or newer than the stored
    /// one. `true` when something was written.
    pub async fn import_attempt(&self, model: ModelId, a: &Attempt) -> Result<bool> {
        if check_attempt(&a.settings, a.duration_min, &a.notes).is_err() {
            return Ok(false);
        }
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, model).await?;
        let existing = sqlx::query!(
            r#"SELECT model_id as "model_id!: ModelId", updated_at FROM attempts WHERE id = ?"#,
            a.id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(e) = existing
            && (e.model_id != model || e.updated_at >= a.updated_at)
        {
            return Ok(false);
        }
        write_attempt(
            &mut tx,
            model,
            a.id,
            &AttemptFields {
                printed_at: a.printed_at,
                result: a.result,
                settings: &a.settings,
                duration_min: a.duration_min,
                notes: &a.notes,
                parts: &a.parts,
            },
            a.updated_at,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Writes an attempt that sync merged, whatever the stored one's time.
    /// `false` when the attempt belongs to another model or is invalid.
    pub(crate) async fn apply_attempt(&self, model: ModelId, a: &Attempt) -> Result<bool> {
        if check_attempt(&a.settings, a.duration_min, &a.notes).is_err() {
            return Ok(false);
        }
        let mut tx = self.writer().begin().await?;
        ensure_model(&mut tx, model).await?;
        let owner = sqlx::query_scalar!(
            r#"SELECT model_id as "m!: ModelId" FROM attempts WHERE id = ?"#,
            a.id
        )
        .fetch_optional(&mut *tx)
        .await?;
        if owner.is_some_and(|m| m != model) {
            return Ok(false);
        }
        write_attempt(
            &mut tx,
            model,
            a.id,
            &AttemptFields {
                printed_at: a.printed_at,
                result: a.result,
                settings: &a.settings,
                duration_min: a.duration_min,
                notes: &a.notes,
                parts: &a.parts,
            },
            a.updated_at,
        )
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Deletes an attempt. Returns its model and the photo files to remove.
    pub async fn delete_attempt(&self, id: AttemptId) -> Result<(ModelId, Vec<PhotoRef>)> {
        let mut tx = self.writer().begin().await?;
        let model = attempt_model(&mut tx, id).await?;
        let photos = sqlx::query!(
            r#"SELECT id as "id!: PhotoId", file_name FROM attempt_photos WHERE attempt_id = ?"#,
            id
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|p| PhotoRef {
            id: p.id,
            path: self.photo_dir().join(id.to_string()).join(p.file_name),
        })
        .collect();
        sqlx::query!("DELETE FROM attempts WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((model, photos))
    }

    /// The model and the full record of attempt `id`.
    pub async fn attempt(&self, id: AttemptId) -> Result<(ModelId, Attempt)> {
        let mut conn = self.writer().acquire().await?;
        let model = attempt_model(&mut conn, id).await?;
        let a = load_attempts(&mut conn, model, self.photo_dir())
            .await?
            .into_iter()
            .find(|a| a.id == id)
            .ok_or(CoreError::AttemptNotFound)?;
        Ok((model, a))
    }

    /// Records a photo file (already copied into place) for attempt `id`.
    pub async fn add_photo_row(
        &self,
        id: AttemptId,
        photo: PhotoId,
        file_name: &str,
    ) -> Result<ModelId> {
        let mut tx = self.writer().begin().await?;
        let model = attempt_model(&mut tx, id).await?;
        let n: i64 = sqlx::query_scalar!(
            r#"SELECT count(*) as "n!: i64" FROM attempt_photos WHERE attempt_id = ?"#,
            id
        )
        .fetch_one(&mut *tx)
        .await?;
        if usize::try_from(n).unwrap_or(usize::MAX) >= MAX_PHOTOS {
            return Err(CoreError::PhotoInvalid);
        }
        let now = now_ms();
        sqlx::query!(
            "INSERT INTO attempt_photos (id, attempt_id, file_name, created_at) VALUES (?, ?, ?, ?)",
            photo,
            id,
            file_name,
            now
        )
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(model)
    }

    /// Forgets photo `id`. Returns its model and file.
    pub async fn remove_photo_row(&self, id: PhotoId) -> Result<(ModelId, PhotoRef)> {
        let mut tx = self.writer().begin().await?;
        let row = sqlx::query!(
            r#"SELECT p.attempt_id as "attempt!: AttemptId", p.file_name, a.model_id as "model!: ModelId"
               FROM attempt_photos p JOIN attempts a ON a.id = p.attempt_id WHERE p.id = ?"#,
            id
        )
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(CoreError::PhotoInvalid)?;
        sqlx::query!("DELETE FROM attempt_photos WHERE id = ?", id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok((
            row.model,
            PhotoRef {
                id,
                path: self
                    .photo_dir()
                    .join(row.attempt.to_string())
                    .join(row.file_name),
            },
        ))
    }

    /// Printer names used in attempts and recipes, most used first.
    pub async fn printers(&self) -> Result<Vec<String>> {
        let rows = sqlx::query_scalar!(
            r#"SELECT p as "p!: String" FROM (
                 SELECT json_extract(settings, '$.printer') as p FROM attempts
                 UNION ALL SELECT json_extract(settings, '$.printer') FROM recipes)
               WHERE p IS NOT NULL AND trim(p) != ''
               GROUP BY lower(p) ORDER BY count(*) DESC, p COLLATE NOCASE LIMIT 50"#
        )
        .fetch_all(self.reader())
        .await?;
        Ok(rows)
    }
}

async fn attempt_model(conn: &mut SqliteConnection, id: AttemptId) -> Result<ModelId> {
    sqlx::query_scalar!(
        r#"SELECT model_id as "m!: ModelId" FROM attempts WHERE id = ?"#,
        id
    )
    .fetch_optional(&mut *conn)
    .await?
    .ok_or(CoreError::AttemptNotFound)
}

async fn write_recipe(conn: &mut SqliteConnection, id: ModelId, r: &Recipe, at: i64) -> Result<()> {
    let material = material_key(&r.settings);
    let settings = json(&r.settings);
    let required = json(&r.required);
    let notes = json(&r.notes);
    let parts = json(&r.parts);
    sqlx::query!(
        "INSERT INTO recipes (model_id, material, settings, required, notes, parts, from_attempt, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
         ON CONFLICT (model_id) DO UPDATE SET material = ?2, settings = ?3, required = ?4, notes = ?5,
           parts = ?6, from_attempt = ?7, updated_at = ?8",
        id,
        material,
        settings,
        required,
        notes,
        parts,
        r.from_attempt,
        at
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

struct AttemptFields<'a> {
    printed_at: i64,
    result: AttemptResult,
    settings: &'a PrintSettings,
    duration_min: Option<u32>,
    notes: &'a str,
    parts: &'a [String],
}

async fn write_attempt(
    conn: &mut SqliteConnection,
    model: ModelId,
    id: AttemptId,
    a: &AttemptFields<'_>,
    at: i64,
) -> Result<()> {
    let result = a.result.as_str();
    let material = material_key(a.settings);
    let settings = json(a.settings);
    let duration = a.duration_min.map(i64::from);
    let parts = json(&a.parts);
    sqlx::query!(
        "INSERT INTO attempts (id, model_id, printed_at, result, material, settings, duration_min, notes, parts, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
         ON CONFLICT (id) DO UPDATE SET printed_at = ?3, result = ?4, material = ?5, settings = ?6,
           duration_min = ?7, notes = ?8, parts = ?9, updated_at = ?10",
        id,
        model,
        a.printed_at,
        result,
        material,
        settings,
        duration,
        a.notes,
        parts,
        at
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{Library, LibraryKind};
    use crate::ids::LibraryId;
    use crate::model_data::{Material, Supports};
    use std::path::PathBuf;

    async fn fx() -> (tempfile::TempDir, Db, ModelId) {
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
        let id = ModelId::new();
        sqlx::query(
            "INSERT INTO models (id, library_id, rel_path, name, kind, created_at, updated_at)
             VALUES (?, ?, 'Bin', 'Bin', 'folder', 1, 1)",
        )
        .bind(id)
        .bind(lib.id)
        .execute(db.writer())
        .await
        .unwrap();
        (d, db, id)
    }

    fn attempt(at: i64, result: AttemptResult) -> NewAttempt {
        NewAttempt {
            printed_at: at,
            result,
            settings: PrintSettings {
                material: Some(Material::Petg),
                printer: Some("Prusa MK4".into()),
                bed_temp: Some(85),
                ..PrintSettings::default()
            },
            duration_min: Some(161),
            notes: String::new(),
            parts: vec![],
        }
    }

    #[tokio::test]
    async fn log_attempt_numbers_in_order() {
        let (_d, db, m) = fx().await;
        let late = db
            .log_attempt(m, &attempt(300, AttemptResult::Success))
            .await
            .unwrap();
        let early = db
            .log_attempt(m, &attempt(100, AttemptResult::Failed))
            .await
            .unwrap();
        let data = db.model_data(m).await.unwrap().unwrap();
        let got: Vec<_> = data.attempts.iter().map(|a| (a.id, a.number)).collect();
        assert_eq!(got, [(late, 2), (early, 1)]);
        assert_eq!(data.attempts[0].duration_min, Some(161));
    }

    #[tokio::test]
    async fn invalid_settings_rejected() {
        let (_d, db, m) = fx().await;
        let mut a = attempt(1, AttemptResult::Success);
        a.settings.bed_temp = Some(850);
        assert!(matches!(
            db.log_attempt(m, &a).await,
            Err(CoreError::SettingOutOfRange {
                field: "bed_temp",
                ..
            })
        ));
        a.settings.bed_temp = None;
        a.duration_min = Some(20_000);
        assert!(db.log_attempt(m, &a).await.is_err());
    }

    #[tokio::test]
    async fn recipe_round_trips_and_is_searchable() {
        let (_d, db, m) = fx().await;
        let r = Recipe {
            settings: PrintSettings {
                material: Some(Material::Petg),
                supports: Some(Supports::Tree),
                ..PrintSettings::default()
            },
            required: ["material".to_owned()].into(),
            notes: vec![RecipeNote {
                material: Some(Material::Petg),
                text: "Warps unless bed at 85".into(),
            }],
            parts: vec![],
            from_attempt: None,
            updated_at: 0,
        };
        db.set_recipe(m, Some(&r)).await.unwrap();
        let got = db.model_data(m).await.unwrap().unwrap().recipe.unwrap();
        assert_eq!(got.settings, r.settings);
        assert_eq!(got.notes, r.notes);
        assert!(got.updated_at > 0);
        let hits = db.search_models("warps", 10).await.unwrap();
        assert_eq!(hits.len(), 1);
        db.set_recipe(m, None).await.unwrap();
        assert!(db.model_data(m).await.unwrap().unwrap().recipe.is_none());
        assert!(db.search_models("warps", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn import_attempt_newest_wins() {
        let (_d, db, m) = fx().await;
        let id = db
            .log_attempt(m, &attempt(1, AttemptResult::Failed))
            .await
            .unwrap();
        let (_, mut a) = db.attempt(id).await.unwrap();
        let older = Attempt {
            notes: "older".into(),
            updated_at: a.updated_at - 1,
            ..a.clone()
        };
        assert!(!db.import_attempt(m, &older).await.unwrap());
        a.notes = "newer".into();
        a.updated_at += 10;
        assert!(db.import_attempt(m, &a).await.unwrap());
        assert_eq!(db.attempt(id).await.unwrap().1.notes, "newer");
    }

    #[tokio::test]
    async fn printers_autocomplete_distinct() {
        let (_d, db, m) = fx().await;
        for _ in 0..2 {
            db.log_attempt(m, &attempt(1, AttemptResult::Success))
                .await
                .unwrap();
        }
        let mut a = attempt(2, AttemptResult::Success);
        a.settings.printer = Some("prusa mk4".into());
        db.log_attempt(m, &a).await.unwrap();
        a.settings.printer = Some("Voron 2.4".into());
        db.log_attempt(m, &a).await.unwrap();
        let p = db.printers().await.unwrap();
        assert_eq!(p.len(), 2);
        assert!(p[0].eq_ignore_ascii_case("prusa mk4"));
        assert_eq!(p[1], "Voron 2.4");
    }

    #[tokio::test]
    async fn photos_capped_and_removed_with_attempt() {
        let (_d, db, m) = fx().await;
        let id = db
            .log_attempt(m, &attempt(1, AttemptResult::Success))
            .await
            .unwrap();
        for i in 0..MAX_PHOTOS {
            db.add_photo_row(id, PhotoId::new(), &format!("{i}.png"))
                .await
                .unwrap();
        }
        assert!(matches!(
            db.add_photo_row(id, PhotoId::new(), "x.png").await,
            Err(CoreError::PhotoInvalid)
        ));
        let (model, photos) = db.delete_attempt(id).await.unwrap();
        assert_eq!(model, m);
        assert_eq!(photos.len(), MAX_PHOTOS);
        assert!(photos[0].path.ends_with(format!("{id}/0.png")));
        assert!(matches!(
            db.attempt(id).await,
            Err(CoreError::AttemptNotFound)
        ));
    }
}
