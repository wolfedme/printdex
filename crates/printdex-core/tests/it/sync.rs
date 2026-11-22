//! Two-way sync end to end: two (or three) engines, each with its own data
//! folder, syncing through a folder space.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use printdex_core::backup::secrets::Store;
use printdex_core::backup::{Target, TargetSecrets};
use printdex_core::sync::library_space_dir;
use printdex_core::{
    AttemptResult, CoreError, Engine, Library, ModelId, NewAttempt, PrintSettings, Recipe,
    SyncError,
};
use tokio_util::sync::CancellationToken;

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

const FILES: &[&str] = &[
    "dragon/body.stl",
    "dragon/wing.stl",
    "castle/keep.stl",
    "boat.stl",
];

/// One install: its own data folder and a library.
struct Install {
    engine: Engine,
    lib: Library,
}

impl Install {
    async fn open(data: &Path, root: &Path) -> Install {
        let engine = Engine::open(data)
            .await
            .unwrap()
            .with_secret_store(Arc::new(Store::with_backend(None)));
        let lib = match engine.library_by_path(root).await.unwrap() {
            Some(l) => l,
            None => engine.add_library(root, None).await.unwrap(),
        };
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        Install { engine, lib }
    }

    async fn model(&self, name: &str) -> ModelId {
        self.engine
            .search(name, 10)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no model {name}"))
            .id
    }

    async fn notes(&self, name: &str) -> String {
        let id = self.model(name).await;
        self.engine.model_data(id).await.unwrap().unwrap().notes
    }

    async fn sync(&self) {
        let reports = self.engine.sync_now().await.unwrap();
        for s in self.engine.sync_status().await.unwrap() {
            assert_eq!(s.last_error, None, "space {} failed: {reports:?}", s.id);
        }
    }

    async fn rescan(&self) {
        self.engine
            .rescan(self.lib.id, CancellationToken::new())
            .await
            .unwrap();
    }
}

/// Two installs with their own copies of the files, syncing through a
/// folder target.
struct OwnCopies {
    dir: tempfile::TempDir,
    a: Install,
    b: Install,
}

impl OwnCopies {
    async fn new(passphrase: &str) -> OwnCopies {
        let dir = tempfile::tempdir().unwrap();
        for who in ["a", "b"] {
            for f in FILES {
                touch(&dir.path().join(who).join("lib"), f);
            }
        }
        let a = Install::open(&dir.path().join("a/data"), &dir.path().join("a/lib")).await;
        let b = Install::open(&dir.path().join("b/data"), &dir.path().join("b/lib")).await;
        let fx = OwnCopies { dir, a, b };
        let target = fx.target();
        let sa =
            fx.a.engine
                .add_target_space(target.clone(), &TargetSecrets::default(), passphrase, true)
                .await
                .unwrap();
        fx.a.engine
            .link_library(sa.id, fx.a.lib.id, None)
            .await
            .unwrap();
        fx.a.sync().await;
        let sb =
            fx.b.engine
                .add_target_space(target, &TargetSecrets::default(), passphrase, true)
                .await
                .unwrap();
        // B reads A's identities first, so it can suggest the match.
        fx.b.sync().await;
        let libs = fx.b.engine.space_libraries(sb.id).await.unwrap();
        assert_eq!(libs.len(), 1);
        assert_eq!(
            libs[0].suggested,
            Some(fx.b.lib.id),
            "fingerprints suggest B's library"
        );
        fx.b.engine
            .link_library(sb.id, fx.b.lib.id, Some(libs[0].id))
            .await
            .unwrap();
        fx.b.sync().await;
        fx.a.sync().await;
        fx
    }

    fn target(&self) -> Target {
        Target::Folder {
            path: self.space_dir(),
        }
    }

    fn space_dir(&self) -> PathBuf {
        self.dir.path().join("space")
    }

    async fn round(&self) {
        self.a.sync().await;
        self.b.sync().await;
        self.a.sync().await;
    }
}

fn attempt(notes: &str) -> NewAttempt {
    NewAttempt {
        printed_at: 1_763_000_000_000,
        result: AttemptResult::Success,
        settings: PrintSettings::default(),
        duration_min: Some(42),
        notes: notes.into(),
        parts: vec![],
    }
}

#[tokio::test]
async fn two_installs_converge() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("dragon").await;
    let db = fx.b.model("dragon").await;
    fx.a.engine.add_tag(da, "Dragon").await.unwrap();
    fx.a.engine.set_notes(da, "Print slow.").await.unwrap();
    fx.b.engine
        .log_attempt(db, &attempt("came out great"))
        .await
        .unwrap();
    fx.round().await;
    for (i, id) in [(&fx.a, da), (&fx.b, db)] {
        let d = i.engine.model_data(id).await.unwrap().unwrap();
        assert_eq!(d.tags, vec!["Dragon".to_string()]);
        assert_eq!(d.notes, "Print slow.");
        assert_eq!(d.attempts.len(), 1);
        assert_eq!(d.attempts[0].notes, "came out great");
        assert_eq!(d.attempts[0].duration_min, Some(42));
    }
    // Removing travels too.
    fx.b.engine.remove_tag(db, "dragon").await.unwrap();
    let att = fx.b.engine.model_data(db).await.unwrap().unwrap().attempts[0].id;
    fx.b.engine.delete_attempt(att).await.unwrap();
    fx.round().await;
    let d = fx.a.engine.model_data(da).await.unwrap().unwrap();
    assert!(d.tags.is_empty());
    assert!(d.attempts.is_empty());
}

#[tokio::test]
async fn recipes_and_collections_travel() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("dragon").await;
    let recipe = Recipe {
        required: ["layer_mm".to_string()].into(),
        settings: PrintSettings {
            layer_mm: Some(0.12),
            ..PrintSettings::default()
        },
        ..Recipe::default()
    };
    fx.a.engine.set_recipe(da, Some(&recipe)).await.unwrap();
    let gifts = fx.a.engine.create_collection("Gifts").await.unwrap();
    fx.a.engine.add_to_collection(gifts.id, da).await.unwrap();
    fx.round().await;
    let db = fx.b.model("dragon").await;
    let d = fx.b.engine.model_data(db).await.unwrap().unwrap();
    let r = d.recipe.expect("recipe synced");
    assert_eq!(r.settings.layer_mm, Some(0.12));
    assert!(r.required.contains("layer_mm"));
    assert_eq!(d.collections.len(), 1);
    assert_eq!(d.collections[0].name, "Gifts");
    // Renaming a collection on B renames it on A, not a new one.
    fx.b.engine
        .rename_collection(d.collections[0].id, "Presents")
        .await
        .unwrap();
    fx.b.engine.set_recipe(db, None).await.unwrap();
    fx.round().await;
    let names: Vec<String> =
        fx.a.engine
            .collections()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
    assert_eq!(names, vec!["Presents".to_string()]);
    assert!(
        fx.a.engine
            .model_data(da)
            .await
            .unwrap()
            .unwrap()
            .recipe
            .is_none()
    );
}

#[tokio::test]
async fn offline_edits_both_survive() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("castle").await;
    let db = fx.b.model("castle").await;
    // Different fields: both kept.
    fx.a.engine.add_tag(da, "terrain").await.unwrap();
    fx.b.engine
        .set_source(db, Some("https://example.com/castle"))
        .await
        .unwrap();
    // Same text field: newer wins, the other is kept.
    fx.a.engine.set_notes(da, "from A").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    fx.b.engine.set_notes(db, "from B").await.unwrap();
    fx.round().await;
    fx.b.sync().await;
    for (i, id) in [(&fx.a, da), (&fx.b, db)] {
        let d = i.engine.model_data(id).await.unwrap().unwrap();
        assert_eq!(d.tags, vec!["terrain".to_string()]);
        assert_eq!(d.source.as_deref(), Some("https://example.com/castle"));
        assert_eq!(d.notes, "from B");
    }
    let history = fx.a.engine.edit_history(da).await.unwrap();
    assert_eq!(history.len(), 1, "A keeps its losing notes");
    assert_eq!(history[0].value, serde_json::json!("from A"));
    assert!(fx.b.engine.edit_history(db).await.unwrap().is_empty());
    // Restoring puts it back everywhere.
    fx.a.engine.restore_edit(history[0].id).await.unwrap();
    fx.round().await;
    assert_eq!(fx.b.notes("castle").await, "from A");
    assert!(fx.a.engine.edit_history(da).await.unwrap().is_empty());
}

#[tokio::test]
async fn sequential_edits_keep_no_history() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("boat").await;
    let db = fx.b.model("boat").await;
    fx.a.engine.set_notes(da, "first").await.unwrap();
    fx.round().await;
    fx.b.sync().await;
    fx.b.engine.set_notes(db, "second").await.unwrap();
    fx.round().await;
    assert_eq!(fx.a.notes("boat").await, "second");
    assert!(fx.a.engine.edit_history(da).await.unwrap().is_empty());
}

#[tokio::test]
async fn catalogue_numbers_agree() {
    let fx = OwnCopies::new("").await;
    for name in ["dragon", "castle", "boat"] {
        let na =
            fx.a.engine
                .model(fx.a.model(name).await)
                .await
                .unwrap()
                .unwrap()
                .number;
        let nb =
            fx.b.engine
                .model(fx.b.model(name).await)
                .await
                .unwrap()
                .unwrap()
                .number;
        assert_eq!(na, nb, "{name}");
    }
}

#[tokio::test]
async fn encrypted_space_roundtrip_and_locked() {
    let fx = OwnCopies::new("correct horse").await;
    let da = fx.a.model("dragon").await;
    fx.a.engine.set_notes(da, "secret notes").await.unwrap();
    fx.round().await;
    assert_eq!(fx.b.notes("dragon").await, "secret notes");
    // Nothing readable in the space.
    for entry in walk(&fx.space_dir()) {
        if entry.extension().is_some_and(|e| e == "chg") {
            let bytes = fs::read(&entry).unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("secret notes"));
        }
    }
    // A third install without the passphrase can't join.
    let c_dir = fx.dir.path().join("c");
    for f in FILES {
        touch(&c_dir.join("lib"), f);
    }
    let c = Install::open(&c_dir.join("data"), &c_dir.join("lib")).await;
    let err = c
        .engine
        .add_target_space(fx.target(), &TargetSecrets::default(), "", true)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Sync(SyncError::PassphraseNeeded)),
        "{err:?}"
    );
    let err = c
        .engine
        .add_target_space(fx.target(), &TargetSecrets::default(), "wrong", true)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CoreError::Sync(SyncError::WrongPassphrase)),
        "{err:?}"
    );
}

#[tokio::test]
async fn locked_space_reports() {
    let fx = OwnCopies::new("pw").await;
    // A fresh engine on A's data has the space but not the passphrase
    // (the test secret store lasts one session).
    let data = fx.dir.path().join("a/data");
    let root = fx.a.lib.path.clone();
    drop(fx.a);
    let a = Install::open(&data, &root).await;
    let status = a.engine.sync_status().await.unwrap();
    assert!(status[0].locked);
    a.engine.sync_now().await.unwrap();
    let status = a.engine.sync_status().await.unwrap();
    assert!(
        status[0]
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("passphrase"),
        "{:?}",
        status[0].last_error
    );
    a.engine
        .unlock_space(status[0].id, "pw", false)
        .await
        .unwrap();
    a.sync().await;
    assert!(!a.engine.sync_status().await.unwrap()[0].locked);
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

#[tokio::test]
async fn bad_batch_keeps_cursor() {
    let fx = OwnCopies::new("").await;
    let db = fx.b.model("dragon").await;
    fx.b.engine.set_notes(db, "B's notes").await.unwrap();
    fx.b.sync().await;
    // Damage B's newest batch.
    let mut batches: Vec<PathBuf> = walk(&fx.space_dir())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "chg"))
        .filter(|p| {
            p.to_string_lossy()
                .contains(&fx.b.engine.install_id().to_string())
        })
        .collect();
    batches.sort();
    let newest = batches.last().unwrap().clone();
    let good = fs::read(&newest).unwrap();
    fs::write(&newest, &good[..good.len() / 2]).unwrap();
    fx.a.engine.sync_now().await.unwrap();
    let st = fx.a.engine.sync_status().await.unwrap();
    assert!(st[0].last_error.is_some());
    assert_eq!(fx.a.notes("dragon").await, "");
    // Fixed: the next sync applies it.
    fs::write(&newest, &good).unwrap();
    fx.a.sync().await;
    assert_eq!(fx.a.notes("dragon").await, "B's notes");
}

#[tokio::test]
async fn steady_state_cheap() {
    let fx = OwnCopies::new("").await;
    let count = || {
        walk(&fx.space_dir())
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "chg"))
            .count()
    };
    let before = count();
    fx.round().await;
    fx.round().await;
    assert_eq!(count(), before, "no batches without changes");
}

#[tokio::test]
async fn photo_travels() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("dragon").await;
    let att = fx.a.engine.log_attempt(da, &attempt("")).await.unwrap();
    let img = fx.dir.path().join("photo.png");
    image::RgbImage::from_pixel(4, 4, image::Rgb([200, 10, 10]))
        .save(&img)
        .unwrap();
    fx.a.engine.add_photo(att, &img).await.unwrap();
    fx.round().await;
    let db = fx.b.model("dragon").await;
    let d = fx.b.engine.model_data(db).await.unwrap().unwrap();
    assert_eq!(d.attempts.len(), 1);
    assert_eq!(d.attempts[0].photos.len(), 1);
    let got = fs::read(&d.attempts[0].photos[0].path).unwrap();
    assert_eq!(got, fs::read(&img).unwrap());
}

#[tokio::test]
async fn parked_until_scan() {
    let fx = OwnCopies::new("").await;
    // A gets a model B doesn't have yet.
    touch(&fx.a.lib.path, "ship/hull.stl");
    fx.a.rescan().await;
    let sa = fx.a.model("ship").await;
    fx.a.engine.set_notes(sa, "needs supports").await.unwrap();
    fx.round().await;
    let space = fx.b.engine.sync_status().await.unwrap()[0].id;
    let parked = fx.b.engine.parked_models(space).await.unwrap();
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].path, "ship");
    // The files arrive on B (say, through Syncthing) and a scan finds them.
    touch(&fx.b.lib.path, "ship/hull.stl");
    fx.b.rescan().await;
    fx.b.sync().await;
    assert_eq!(fx.b.notes("ship").await, "needs supports");
    assert!(fx.b.engine.parked_models(space).await.unwrap().is_empty());
}

#[tokio::test]
async fn moved_folder_keeps_data() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("castle").await;
    fx.a.engine.set_notes(da, "big print").await.unwrap();
    fx.round().await;
    // B had moved its castle folder before; the fingerprints still match.
    let b_root = fx.b.lib.path.clone();
    fs::rename(b_root.join("castle"), b_root.join("fort")).unwrap();
    fx.b.rescan().await;
    assert_eq!(fx.b.notes("fort").await, "big print");
    fx.a.engine.set_notes(da, "bigger print").await.unwrap();
    fx.round().await;
    assert_eq!(fx.b.notes("fort").await, "bigger print");
}

#[tokio::test]
async fn shared_library_no_sidecar_echo() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("nas");
    for f in FILES {
        touch(&root, f);
    }
    let a = Install::open(&dir.path().join("a"), &root).await;
    let b = Install::open(&dir.path().join("b"), &root).await;
    a.engine.add_library_space(a.lib.id, "").await.unwrap();
    a.sync().await;
    // B joins the same library's space.
    b.engine.add_library_space(b.lib.id, "").await.unwrap();
    b.sync().await;
    a.sync().await;
    assert!(library_space_dir(&root).join("space.toml").exists());
    let da = a.model("dragon").await;
    a.engine.set_notes(da, "shared notes").await.unwrap();
    a.engine.flush_sidecars().await;
    a.sync().await;
    b.sync().await;
    assert_eq!(b.notes("dragon").await, "shared notes");
    // B rescans and sees A's sidecar: no new change from B.
    b.engine.flush_sidecars().await;
    b.rescan().await;
    let reports = b.engine.sync_now().await.unwrap();
    assert_eq!(reports[0].sent, 0, "{reports:?}");
    assert_eq!(b.notes("dragon").await, "shared notes");
}

#[tokio::test]
async fn forget_device_rules() {
    let fx = OwnCopies::new("").await;
    let st = fx.a.engine.sync_status().await.unwrap();
    let b_id = fx.b.engine.install_id();
    assert!(st[0].devices.iter().any(|d| d.install == b_id));
    let err = fx.a.engine.forget_device(st[0].id, b_id).await.unwrap_err();
    assert!(matches!(err, CoreError::Sync(SyncError::DeviceRecent)));
}

#[tokio::test]
async fn compaction_keeps_new_installs_whole() {
    let fx = OwnCopies::new("").await;
    let da = fx.a.model("dragon").await;
    // Enough changes for two snapshots.
    fx.a.engine.set_sync_snapshot_after(5);
    for i in 0..30 {
        fx.a.engine
            .set_notes(da, &format!("rev {i}"))
            .await
            .unwrap();
        fx.a.engine
            .add_tag(da, &format!("t{}", i % 3))
            .await
            .unwrap();
        fx.a.sync().await;
    }
    fx.a.engine.set_notes(da, "final").await.unwrap();
    fx.a.sync().await;
    let a_logs = walk(&fx.space_dir())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "chg"))
        .filter(|p| {
            p.to_string_lossy()
                .contains(&fx.a.engine.install_id().to_string())
        })
        .count();
    assert!(a_logs < 10, "old batches were removed ({a_logs} left)");
    let snaps = walk(&fx.space_dir())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "snap"))
        .count();
    assert!(snaps >= 1, "a snapshot was written");
    // A new install joins and gets everything.
    let c_dir = fx.dir.path().join("c");
    for f in FILES {
        touch(&c_dir.join("lib"), f);
    }
    let c = Install::open(&c_dir.join("data"), &c_dir.join("lib")).await;
    let s = c
        .engine
        .add_target_space(fx.target(), &TargetSecrets::default(), "", true)
        .await
        .unwrap();
    c.sync().await;
    let lib = c.engine.space_libraries(s.id).await.unwrap()[0].id;
    c.engine
        .link_library(s.id, c.lib.id, Some(lib))
        .await
        .unwrap();
    c.sync().await;
    assert_eq!(c.notes("dragon").await, "final");
    let d = c
        .engine
        .model_data(c.model("dragon").await)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(d.tags.len(), 3);
}
