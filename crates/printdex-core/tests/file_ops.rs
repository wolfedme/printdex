//! Rename, move and trash with the undo journal.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use printdex_core::db::Db;
use printdex_core::db::file_ops::{How, OpKind, Step};
use printdex_core::fs::ChaosFs;
use printdex_core::{CoreError, Engine, Library, ModelId, ModelKind, OpState};
use tokio_util::sync::CancellationToken;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    lib: Library,
}

impl Fx {
    async fn new(files: &[&str]) -> Fx {
        Fx::with_fs(files, Arc::new(printdex_core::fs::RealFs)).await
    }

    async fn with_fs(files: &[&str], fs_: Arc<dyn printdex_core::fs::Fs>) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        fs::create_dir_all(&root).unwrap();
        for f in files {
            write(&root, f, f);
        }
        let data = dir.path().join("data");
        let engine = Engine::open_with_fs(&data, &data.join("cache"), fs_)
            .await
            .unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        Fx { dir, engine, lib }
    }

    fn root(&self) -> &Path {
        &self.lib.path
    }

    async fn find(&self, name: &str) -> ModelId {
        self.engine
            .search(name, 50)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no model {name}"))
            .id
    }

    async fn second_library(&self, files: &[&str]) -> Library {
        let root = self.dir.path().join("lib2");
        fs::create_dir_all(&root).unwrap();
        for f in files {
            write(&root, f, f);
        }
        let lib = self.engine.add_library(&root, None).await.unwrap();
        self.engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        lib
    }
}

#[tokio::test]
async fn rename_folder_model_moves_rows_and_sidecar() {
    let f = Fx::new(&["Hook/hook.stl", "Hook/photo.jpg"]).await;
    let id = f.find("Hook").await;
    f.engine.add_tag(id, "keeper").await.unwrap();
    fs::write(f.root().join("Hook/.printdex.toml"), "x = 1\n").unwrap();
    let op = f.engine.rename_model(id, "Wall hook").await.unwrap();
    assert_eq!(op.kind, OpKind::Rename);
    assert!(op.undoable);
    assert!(f.root().join("Wall hook/hook.stl").exists());
    assert!(f.root().join("Wall hook/.printdex.toml").exists());
    assert!(!f.root().join("Hook").exists());
    let d = f.engine.model(id).await.unwrap().unwrap();
    assert_eq!(d.rel_path, "Wall hook");
    assert_eq!(d.name, "Wall hook");
    assert_eq!(d.parts[0].rel_path, "Wall hook/hook.stl");
    assert_eq!(d.assets[0].rel_path, "Wall hook/photo.jpg");
    // A rescan agrees with the database: nothing new, nothing missing.
    let s = f
        .engine
        .rescan(f.lib.id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!((s.models_added, s.models_missing), (0, 0));
    assert_eq!(f.find("Wall hook").await, id);

    // Undo puts it back.
    f.engine.undo(op.id).await.unwrap();
    assert!(f.root().join("Hook/hook.stl").exists());
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
    // Only once.
    assert!(matches!(
        f.engine.undo(op.id).await,
        Err(CoreError::UndoUnavailable)
    ));
    assert_eq!(f.engine.last_op().await.unwrap(), None);
}

#[tokio::test]
async fn rename_single_file_moves_its_sidecar() {
    let f = Fx::new(&["hook.stl", "Other/x.stl"]).await;
    let id = f.find("hook").await;
    fs::write(f.root().join(".hook.stl.printdex.toml"), "x = 1\n").unwrap();
    f.engine.rename_model(id, "Hanger").await.unwrap();
    assert!(f.root().join("Hanger.stl").exists());
    assert!(f.root().join(".Hanger.stl.printdex.toml").exists());
    assert!(!f.root().join(".hook.stl.printdex.toml").exists());
    let d = f.engine.model(id).await.unwrap().unwrap();
    assert_eq!(
        (d.rel_path.as_str(), d.name.as_str()),
        ("Hanger.stl", "Hanger")
    );
}

#[tokio::test]
async fn bad_names_are_refused() {
    let f = Fx::new(&["Hook/hook.stl"]).await;
    let id = f.find("Hook").await;
    for bad in ["", "a/b", "CON", "x?"] {
        assert!(
            matches!(
                f.engine.rename_model(id, bad).await,
                Err(CoreError::NameInvalid)
            ),
            "{bad:?}"
        );
    }
    assert!(f.root().join("Hook/hook.stl").exists());
}

#[tokio::test]
async fn move_collision_refused() {
    let f = Fx::new(&["Hook/hook.stl", "Bins/Hook/other.stl", "Bins/Bin/bin.stl"]).await;
    let id = f.find("Hook").await;
    let r = f.engine.move_models(&[id], f.lib.id, "Bins").await;
    assert!(matches!(r, Err(CoreError::NameTaken(n)) if n == "Hook"));
    // Nothing touched.
    assert!(f.root().join("Hook/hook.stl").exists());
    assert!(f.root().join("Bins/Hook/other.stl").exists());
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
    // Into itself is refused too.
    assert!(matches!(
        f.engine.move_models(&[id], f.lib.id, "Hook/sub").await,
        Err(CoreError::NameInvalid)
    ));
}

#[tokio::test]
async fn move_within_library_into_folder() {
    let f = Fx::new(&["Hook/hook.stl", "Vase/vase.stl", "Storage/Bin/bin.stl"]).await;
    let hook = f.find("Hook").await;
    let vase = f.find("Vase").await;
    let number = f.engine.model(hook).await.unwrap().unwrap().number;
    let op = f
        .engine
        .move_models(&[hook, vase], f.lib.id, "Storage")
        .await
        .unwrap();
    assert_eq!(op.names, ["Hook", "Vase"]);
    assert!(f.root().join("Storage/Hook/hook.stl").exists());
    assert!(f.root().join("Storage/Vase/vase.stl").exists());
    let d = f.engine.model(hook).await.unwrap().unwrap();
    assert_eq!(d.rel_path, "Storage/Hook");
    assert_eq!(d.number, number, "same library keeps its number");
    f.engine.undo(op.id).await.unwrap();
    assert!(f.root().join("Hook/hook.stl").exists());
    assert!(f.root().join("Vase/vase.stl").exists());
}

#[tokio::test]
async fn move_across_libraries_copies_then_trashes() {
    // Two folders on one disk rename; the copy path is what a second
    // drive takes, and both end the same way for the database.
    let f = Fx::new(&["Hook/hook.stl", "Other/x.stl"]).await;
    let other = f.second_library(&["Bins/bin.stl"]).await;
    let id = f.find("Hook").await;
    f.engine.add_tag(id, "keeper").await.unwrap();
    f.engine.move_models(&[id], other.id, "").await.unwrap();
    assert!(other.path.join("Hook/hook.stl").exists());
    assert!(!f.root().join("Hook").exists());
    let d = f.engine.model(id).await.unwrap().unwrap();
    assert_eq!(d.library_id, other.id);
    assert_eq!(d.rel_path, "Hook");
    assert_eq!(d.number, Some(2), "a new number in the new library");
    let data = f.engine.model_data(id).await.unwrap().unwrap();
    assert_eq!(data.tags, ["keeper"]);
    let s = f
        .engine
        .rescan(other.id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!((s.models_added, s.models_missing), (0, 0));
}

#[tokio::test]
async fn offline_target_refused() {
    let chaos = Arc::new(ChaosFs::new());
    let f = Fx::with_fs(&["Hook/hook.stl"], chaos.clone()).await;
    let other = f.second_library(&["Bins/bin.stl"]).await;
    let id = f.find("Hook").await;
    chaos.unmount(&other.path);
    assert!(matches!(
        f.engine.move_models(&[id], other.id, "").await,
        Err(CoreError::TargetOffline)
    ));
    assert!(f.root().join("Hook/hook.stl").exists());
}

#[tokio::test]
async fn trash_then_undo_restores() {
    let f = Fx::new(&["Hook/hook.stl", "Plain/plain.stl", "Other/x.stl"]).await;
    let hook = f.find("Hook").await;
    let plain = f.find("Plain").await;
    f.engine.add_tag(hook, "keeper").await.unwrap();
    let op = f.engine.trash_models(&[hook, plain]).await.unwrap();
    assert!(!f.root().join("Hook").exists());
    assert!(!f.root().join("Plain").exists());
    // With user data: kept under Missing. Without: forgotten.
    let d = f.engine.model(hook).await.unwrap().unwrap();
    assert!(d.missing_since.is_some());
    assert_eq!(f.engine.model(plain).await.unwrap(), None);

    if !printdex_core::file_ops::CAN_RESTORE {
        assert!(matches!(
            f.engine.undo(op.id).await,
            Err(CoreError::UndoUnavailable)
        ));
        return;
    }
    f.engine.undo(op.id).await.unwrap();
    assert!(f.root().join("Hook/hook.stl").exists());
    assert!(f.root().join("Plain/plain.stl").exists());
    let d = f.engine.model(hook).await.unwrap().unwrap();
    assert!(d.missing_since.is_none(), "the tagged model is back");
    assert_eq!(
        f.engine.model_data(hook).await.unwrap().unwrap().tags,
        ["keeper"]
    );
    // The forgotten one returns as a new model.
    let back = f.find("Plain").await;
    assert_ne!(back, plain);
}

#[tokio::test]
async fn planned_op_reconciled_on_start() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    write(&root, "Hook/hook.stl", "hook");
    write(&root, "Vase/vase.stl", "vase");
    let data = dir.path().join("data");
    let (id, vase, lib) = {
        let engine = Engine::open(&data).await.unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        let find = |n: &'static str| {
            let engine = engine.clone();
            async move {
                engine
                    .search(n, 10)
                    .await
                    .unwrap()
                    .into_iter()
                    .find(|h| h.name == n)
                    .unwrap()
                    .id
            }
        };
        (find("Hook").await, find("Vase").await, lib)
    };
    // A rename that was journaled; the crash came after the files moved
    // for Hook and before anything happened to Vase.
    let db = Db::open(&data.join("printdex.db")).await.unwrap();
    let step = |model, name: &str, to: &str| Step {
        model,
        kind: ModelKind::Folder,
        name: name.into(),
        new_name: to.into(),
        from_lib: lib.id,
        from_rel: name.into(),
        to_lib: lib.id,
        to_rel: to.into(),
        how: How::Rename,
    };
    let op = db
        .insert_op(
            OpKind::Move,
            &[
                step(id, "Hook", "Wall hook"),
                step(vase, "Vase", "Big vase"),
            ],
        )
        .await
        .unwrap();
    fs::rename(root.join("Hook"), root.join("Wall hook")).unwrap();
    drop(db);

    let engine = Engine::open(&data).await.unwrap();
    assert_eq!(engine.take_recovered(), ["Wall hook"]);
    assert!(engine.take_recovered().is_empty(), "told once");
    let d = engine.model(id).await.unwrap().unwrap();
    assert_eq!(d.rel_path, "Wall hook");
    assert_eq!(
        engine.model(vase).await.unwrap().unwrap().rel_path,
        "Vase",
        "the step that never ran changes nothing"
    );
    let db = Db::open(&data.join("printdex.db")).await.unwrap();
    let last = db.last_op().await.unwrap().unwrap();
    assert_eq!((last.id, last.state), (op, OpState::Done));
    let _: PathBuf = root;
}
