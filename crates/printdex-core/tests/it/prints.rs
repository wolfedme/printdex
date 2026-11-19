//! Recipe and print-log commands through the engine.

use std::fs;

use printdex_core::{AttemptResult, Engine, Material, NewAttempt, PrintSettings, Supports};
use tokio_util::sync::CancellationToken;

async fn fx() -> (tempfile::TempDir, Engine, printdex_core::ModelId) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib/Bin");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.stl"), b"x").unwrap();
    let engine = Engine::open(&dir.path().join("data")).await.unwrap();
    let lib = engine
        .add_library(&dir.path().join("lib"), None)
        .await
        .unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let id = engine.search("bin", 1).await.unwrap()[0].id;
    (dir, engine, id)
}

fn attempt(result: AttemptResult) -> NewAttempt {
    NewAttempt {
        printed_at: 1_763_000_000_000,
        result,
        settings: PrintSettings {
            material: Some(Material::Petg),
            supports: Some(Supports::Tree),
            bed_temp: Some(85),
            ..PrintSettings::default()
        },
        duration_min: None,
        notes: String::new(),
        parts: vec![],
    }
}

#[tokio::test]
async fn recipe_from_attempt_copies_settings() {
    let (_d, engine, m) = fx().await;
    let a = engine
        .log_attempt(m, &attempt(AttemptResult::Success))
        .await
        .unwrap();
    engine.recipe_from_attempt(a).await.unwrap();
    let r = engine.model_data(m).await.unwrap().unwrap().recipe.unwrap();
    assert_eq!(r.settings.bed_temp, Some(85));
    assert_eq!(r.from_attempt, Some(a));
    let hit = &engine.search("bin", 1).await.unwrap()[0];
    assert_eq!(hit.material, Some(Material::Petg));
}

#[tokio::test]
async fn note_from_failed_attempt_is_scoped_to_material() {
    let (_d, engine, m) = fx().await;
    let a = engine
        .log_attempt(m, &attempt(AttemptResult::Failed))
        .await
        .unwrap();
    engine.note_from_attempt(a, "Needs a brim").await.unwrap();
    let r = engine.model_data(m).await.unwrap().unwrap().recipe.unwrap();
    assert_eq!(r.notes.len(), 1);
    assert_eq!(r.notes[0].material, Some(Material::Petg));
    assert_eq!(r.notes[0].text, "Needs a brim");
    assert!(r.settings.is_empty());
}

#[tokio::test]
async fn photo_copied_and_deleted_with_attempt() {
    let (d, engine, m) = fx().await;
    let a = engine
        .log_attempt(m, &attempt(AttemptResult::Success))
        .await
        .unwrap();
    let src = d.path().join("photo.png");
    let mut png = std::io::Cursor::new(Vec::new());
    image::RgbaImage::new(4, 4)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    fs::write(&src, png.into_inner()).unwrap();
    let photo = engine.add_photo(a, &src).await.unwrap();
    assert!(photo.path.is_file());
    assert!(photo.path.starts_with(d.path().join("data/photos")));
    let data = engine.model_data(m).await.unwrap().unwrap();
    assert_eq!(data.attempts[0].photos, vec![photo.clone()]);
    engine.delete_attempt(a).await.unwrap();
    assert!(!photo.path.exists());
    assert!(
        engine
            .model_data(m)
            .await
            .unwrap()
            .unwrap()
            .attempts
            .is_empty()
    );
}

#[tokio::test]
async fn photo_must_be_an_image() {
    let (d, engine, m) = fx().await;
    let a = engine
        .log_attempt(m, &attempt(AttemptResult::Success))
        .await
        .unwrap();
    let src = d.path().join("notes.png");
    fs::write(&src, b"not an image").unwrap();
    assert!(matches!(
        engine.add_photo(a, &src).await,
        Err(printdex_core::CoreError::PhotoInvalid)
    ));
}

#[tokio::test]
async fn set_material_keeps_the_rest_of_the_recipe() {
    let (_d, engine, m) = fx().await;
    let a = engine
        .log_attempt(m, &attempt(AttemptResult::Success))
        .await
        .unwrap();
    engine.recipe_from_attempt(a).await.unwrap();
    assert_eq!(
        engine
            .set_material(&[m], Some(Material::Tpu))
            .await
            .unwrap(),
        1
    );
    let r = engine.model_data(m).await.unwrap().unwrap().recipe.unwrap();
    assert_eq!(r.settings.material, Some(Material::Tpu));
    assert_eq!(r.settings.bed_temp, Some(85));
    // Setting it again changes nothing.
    assert_eq!(
        engine
            .set_material(&[m], Some(Material::Tpu))
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn set_material_on_a_model_without_recipe() {
    let (_d, engine, m) = fx().await;
    engine
        .set_material(&[m], Some(Material::Pla))
        .await
        .unwrap();
    let r = engine.model_data(m).await.unwrap().unwrap().recipe.unwrap();
    assert_eq!(r.settings.material, Some(Material::Pla));
    // Clearing the only setting removes the recipe.
    engine.set_material(&[m], None).await.unwrap();
    assert!(
        engine
            .model_data(m)
            .await
            .unwrap()
            .unwrap()
            .recipe
            .is_none()
    );
}
