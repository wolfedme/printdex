//! Models keep their identity (and everything the user wrote) when their
//! files move or are renamed between scans.

use std::fs;
use std::path::Path;

use printdex_core::{AttemptResult, Engine, Library, ModelId, NewAttempt, PrintSettings};
use tokio_util::sync::CancellationToken;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

struct Fx {
    _dir: tempfile::TempDir,
    engine: Engine,
    lib: Library,
}

impl Fx {
    async fn new(files: &[(&str, &str)]) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        fs::create_dir_all(&root).unwrap();
        for (f, body) in files {
            write(&root, f, body);
        }
        let engine = Engine::open(&dir.path().join("data")).await.unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        let fx = Fx {
            _dir: dir,
            engine,
            lib,
        };
        fx.scan().await;
        fx
    }

    fn root(&self) -> &Path {
        &self.lib.path
    }

    async fn scan(&self) -> printdex_core::ScanSummary {
        self.engine
            .rescan(self.lib.id, CancellationToken::new())
            .await
            .unwrap()
    }

    async fn find(&self, name: &str) -> Option<ModelId> {
        self.engine
            .search(name, 50)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .map(|h| h.id)
    }

    async fn rel_path(&self, id: ModelId) -> Option<String> {
        self.engine.model(id).await.unwrap().map(|m| m.rel_path)
    }

    /// Tags, a recipe-less attempt: the data that must follow a move.
    async fn decorate(&self, id: ModelId) {
        self.engine.add_tag(id, "keeper").await.unwrap();
        self.engine
            .log_attempt(
                id,
                &NewAttempt {
                    printed_at: 1_763_000_000_000,
                    result: AttemptResult::Success,
                    settings: PrintSettings::default(),
                    duration_min: None,
                    notes: String::new(),
                    parts: vec![],
                },
            )
            .await
            .unwrap();
    }

    async fn assert_decorated(&self, id: ModelId) {
        let d = self.engine.model_data(id).await.unwrap().unwrap();
        assert_eq!(d.tags, ["keeper"]);
        assert_eq!(d.attempts.len(), 1);
    }
}

#[tokio::test]
async fn renamed_folder_keeps_identity() {
    let f = Fx::new(&[
        ("Dragon/body.stl", "body"),
        ("Dragon/wing.stl", "wing"),
        ("Other/x.stl", "x"),
    ])
    .await;
    let id = f.find("Dragon").await.unwrap();
    f.decorate(id).await;
    let number = f.engine.model(id).await.unwrap().unwrap().number;

    fs::rename(f.root().join("Dragon"), f.root().join("Red Dragon")).unwrap();
    let s = f.scan().await;
    assert_eq!(s.models_moved, 1);
    assert_eq!(s.models_added, 0);
    assert_eq!(s.models_missing, 0);

    assert_eq!(f.find("Red Dragon").await, Some(id));
    assert_eq!(f.rel_path(id).await.as_deref(), Some("Red Dragon"));
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().number, number);
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().parts.len(), 2);
    f.assert_decorated(id).await;
}

#[tokio::test]
async fn moved_folder_into_another_folder_keeps_identity() {
    let f = Fx::new(&[("Dragon/body.stl", "body"), ("Dragon/wing.stl", "wing")]).await;
    let id = f.find("Dragon").await.unwrap();
    f.decorate(id).await;
    fs::create_dir_all(f.root().join("Archive/Fantasy")).unwrap();
    fs::rename(
        f.root().join("Dragon"),
        f.root().join("Archive/Fantasy/Dragon"),
    )
    .unwrap();
    f.scan().await;
    assert_eq!(
        f.rel_path(id).await.as_deref(),
        Some("Archive/Fantasy/Dragon")
    );
    f.assert_decorated(id).await;
}

#[tokio::test]
async fn moved_single_file_keeps_identity() {
    // Files at the library root are single-file models.
    let f = Fx::new(&[("hook.stl", "hook"), ("clip.stl", "clip")]).await;
    let id = f.find("hook").await.unwrap();
    f.decorate(id).await;
    f.engine.flush_sidecars().await;
    fs::rename(f.root().join("hook.stl"), f.root().join("wall-hook.stl")).unwrap();
    f.engine.flush_sidecars().await;
    assert!(f.root().join(".hook.stl.printdex.toml").exists());
    let s = f.scan().await;
    assert_eq!(s.models_moved, 1);
    assert_eq!(f.find("wall-hook").await, Some(id));
    f.assert_decorated(id).await;
    f.engine.flush_sidecars().await;
    assert!(!f.root().join(".hook.stl.printdex.toml").exists());
    assert!(f.root().join(".wall-hook.stl.printdex.toml").exists());
}

#[tokio::test]
async fn folder_with_one_part_edited_still_matches() {
    let parts: Vec<(String, String)> = (0..5)
        .map(|i| (format!("Gearbox/p{i}.stl"), format!("part {i}")))
        .collect();
    let refs: Vec<(&str, &str)> = parts
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let f = Fx::new(&refs).await;
    let id = f.find("Gearbox").await.unwrap();
    f.decorate(id).await;
    fs::rename(f.root().join("Gearbox"), f.root().join("Gearbox v2")).unwrap();
    write(f.root(), "Gearbox v2/p4.stl", "part 4, edited");
    f.scan().await;
    assert_eq!(f.find("Gearbox v2").await, Some(id));
    f.assert_decorated(id).await;
}

#[tokio::test]
async fn duplicate_copies_are_not_merged() {
    let f = Fx::new(&[("Dragon/body.stl", "body"), ("Dragon/wing.stl", "wing")]).await;
    let id = f.find("Dragon").await.unwrap();
    f.decorate(id).await;

    // A copy next to the original: a new model, the original untouched.
    write(f.root(), "Dragon copy/body.stl", "body");
    write(f.root(), "Dragon copy/wing.stl", "wing");
    let s = f.scan().await;
    assert_eq!((s.models_added, s.models_moved), (1, 0));
    assert_eq!(f.rel_path(id).await.as_deref(), Some("Dragon"));
    let copy = f.find("Dragon copy").await.unwrap();
    assert_ne!(copy, id);

    // The original moves away while a second copy appears: two equally
    // good matches for the original, so nothing is merged.
    fs::rename(f.root().join("Dragon"), f.root().join("Dragon A")).unwrap();
    write(f.root(), "Dragon B/body.stl", "body");
    write(f.root(), "Dragon B/wing.stl", "wing");
    let s = f.scan().await;
    assert_eq!(s.models_moved, 0);
    assert_eq!(
        f.engine.model(id).await.unwrap(),
        None,
        "original is missing"
    );
    let a = f.find("Dragon A").await.unwrap();
    assert_ne!(a, id);
    // The copy's data is its own.
    assert!(
        f.engine
            .model_data(copy)
            .await
            .unwrap()
            .unwrap()
            .tags
            .is_empty()
    );
}

#[tokio::test]
async fn missing_model_comes_back_at_a_new_path() {
    let f = Fx::new(&[("Vase/vase.stl", "vase"), ("Other/x.stl", "x")]).await;
    let id = f.find("Vase").await.unwrap();
    f.decorate(id).await;
    // Moved out of the library for a while...
    let outside = f._dir.path().join("vase-backup");
    fs::rename(f.root().join("Vase"), &outside).unwrap();
    f.scan().await;
    assert_eq!(f.engine.model(id).await.unwrap(), None);
    // ...then back under another name.
    fs::rename(&outside, f.root().join("Twisted vase")).unwrap();
    f.scan().await;
    assert_eq!(f.find("Twisted vase").await, Some(id));
    f.assert_decorated(id).await;
}

#[tokio::test]
async fn moved_folder_sidecar_follows() {
    let f = Fx::new(&[("Dragon/body.stl", "body")]).await;
    let id = f.find("Dragon").await.unwrap();
    f.decorate(id).await;
    f.engine.flush_sidecars().await;
    assert!(f.root().join("Dragon/.printdex.toml").exists());
    fs::rename(f.root().join("Dragon"), f.root().join("Wyrm")).unwrap();
    f.scan().await;
    f.engine.flush_sidecars().await;
    assert_eq!(f.find("Wyrm").await, Some(id));
    let side = fs::read_to_string(f.root().join("Wyrm/.printdex.toml")).unwrap();
    assert!(side.contains(&id.to_string()), "{side}");
}
