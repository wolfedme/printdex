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

/// Deliberately uses the real system trash (no `Op::Trash` fault), to check
/// a trash and its undo against the platform's trash. Other tests route the trash into a folder with
/// `faults::arm(.., Op::Trash, .., Action::Instead(..))`.
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

// ---- Never overwrite, survive a failure at any step ----

use printdex_core::fsx::faults::{self, Action, FaultGuard, Op};
use std::io::ErrorKind;

/// The state of the newest journaled file action, read from the database.
async fn last_op_state(f: &Fx) -> OpState {
    let db = Db::open(&f.dir.path().join("data/printdex.db"))
        .await
        .unwrap();
    db.last_op().await.unwrap().unwrap().state
}

/// Makes every rename in the fixture's library report another drive, so
/// moves take the copy branch, and sends what they trash to a
/// `.test-trash` folder in the fixture instead of the system trash.
fn cross_drive(f: &Fx) -> Vec<FaultGuard> {
    let root = f.root();
    let bin = f.dir.path().join(".test-trash");
    let mut guards: Vec<FaultGuard> = (0..64)
        .map(|k| faults::arm(root, Op::Rename, k, Action::Fail(ErrorKind::CrossesDevices)))
        .collect();
    guards.extend((0..64).map(|k| {
        let bin = bin.clone();
        faults::arm(
            root,
            Op::Trash,
            k,
            Action::Instead(Box::new(move |p| {
                fs::create_dir_all(&bin)?;
                fs::rename(p, bin.join(k.to_string()))
            })),
        )
    }));
    guards
}

#[tokio::test]
async fn move_two_models_differing_only_in_case_is_refused() {
    let f = Fx::new(&[
        "A/Benchy.stl",
        "A/KeepA/a.stl",
        "B/benchy.stl",
        "B/KeepB/b.stl",
    ])
    .await;
    let upper = f.find("Benchy").await;
    let lower = f.find("benchy").await;
    let r = f.engine.move_models(&[upper, lower], f.lib.id, "C").await;
    assert!(matches!(r, Err(CoreError::NameTaken(_))), "{r:?}");
    assert!(f.root().join("A/Benchy.stl").exists());
    assert!(f.root().join("B/benchy.stl").exists());
    assert!(!f.root().join("C").exists());
    assert_eq!(
        f.engine.model(upper).await.unwrap().unwrap().rel_path,
        "A/Benchy.stl"
    );
}

#[tokio::test]
async fn file_appearing_at_target_is_never_overwritten() {
    let f = Fx::new(&["hook.stl", "Storage/Bin/bin.stl"]).await;
    let id = f.find("hook").await;
    let _g = faults::arm(
        f.root(),
        Op::Rename,
        0,
        Action::Before(Box::new(|to| fs::write(to, "theirs").unwrap())),
    );
    let r = f.engine.move_models(&[id], f.lib.id, "Storage").await;
    assert!(
        matches!(r, Err(CoreError::NameTaken(ref n)) if n == "hook.stl"),
        "{r:?}"
    );
    assert_eq!(
        fs::read_to_string(f.root().join("Storage/hook.stl")).unwrap(),
        "theirs"
    );
    assert_eq!(
        fs::read_to_string(f.root().join("hook.stl")).unwrap(),
        "hook.stl"
    );
    assert_eq!(
        f.engine.model(id).await.unwrap().unwrap().rel_path,
        "hook.stl"
    );
    assert_eq!(last_op_state(&f).await, OpState::Failed);
}

#[cfg(unix)]
#[tokio::test]
async fn cross_drive_move_with_symlink_is_refused() {
    let f = Fx::new(&["Hook/hook.stl", "Storage/Bin/bin.stl"]).await;
    let id = f.find("Hook").await;
    std::os::unix::fs::symlink("hook.stl", f.root().join("Hook/link")).unwrap();
    let _cross = cross_drive(&f);
    let r = f.engine.move_models(&[id], f.lib.id, "Storage").await;
    assert!(matches!(r, Err(CoreError::HasLinks(_))), "{r:?}");
    assert_eq!(
        fs::read_to_string(f.root().join("Hook/hook.stl")).unwrap(),
        "Hook/hook.stl"
    );
    assert!(
        fs::symlink_metadata(f.root().join("Hook/link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(!f.root().join("Storage/Hook").exists());
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
}

#[tokio::test]
async fn cross_drive_move_full_target_cleans_up() {
    let f = Fx::new(&["Hook/a.stl", "Hook/b.stl", "Storage/Bin/bin.stl"]).await;
    let id = f.find("Hook").await;
    let full = faults::arm(f.root(), Op::Copy, 1, Action::Fail(ErrorKind::StorageFull));
    let _cross = cross_drive(&f);
    let r = f.engine.move_models(&[id], f.lib.id, "Storage").await;
    assert!(r.is_err());
    assert_eq!(full.fired(), 1);
    for p in ["Hook/a.stl", "Hook/b.stl"] {
        assert_eq!(fs::read_to_string(f.root().join(p)).unwrap(), p);
    }
    assert!(!f.root().join("Storage/Hook").exists());
    assert_eq!(f.engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
}

#[tokio::test]
async fn rollback_that_fails_leaves_db_matching_disk() {
    let f = Fx::new(&["Hook/hook.stl", "Vase/vase.stl", "Storage/Bin/bin.stl"]).await;
    let hook = f.find("Hook").await;
    let vase = f.find("Vase").await;
    // Vase's move fails, and so does putting Hook back.
    let _a = faults::arm(f.root(), Op::Rename, 1, Action::Fail(ErrorKind::Other));
    let _b = faults::arm(f.root(), Op::Rename, 2, Action::Fail(ErrorKind::Other));
    let r = f
        .engine
        .move_models(&[hook, vase], f.lib.id, "Storage")
        .await;
    assert!(r.is_err());
    assert!(f.root().join("Storage/Hook/hook.stl").exists());
    assert!(f.root().join("Vase/vase.stl").exists());
    assert_eq!(
        f.engine.model(hook).await.unwrap().unwrap().rel_path,
        "Storage/Hook"
    );
    assert_eq!(
        f.engine.model(vase).await.unwrap().unwrap().rel_path,
        "Vase"
    );
}

#[tokio::test]
async fn undo_never_overwrites_what_took_the_old_name() {
    let f = Fx::new(&["Hook/hook.stl", "Other/x.stl"]).await;
    let id = f.find("Hook").await;
    let op = f.engine.rename_model(id, "Wall hook").await.unwrap();
    write(f.root(), "Hook/new.stl", "new");
    let r = f.engine.undo(op.id).await;
    assert!(
        matches!(r, Err(CoreError::NameTaken(ref n)) if n == "Hook"),
        "{r:?}"
    );
    assert_eq!(
        fs::read_to_string(f.root().join("Hook/new.stl")).unwrap(),
        "new"
    );
    assert!(f.root().join("Wall hook/hook.stl").exists());
}

/// Undo of a move after the user renamed the moved folder by hand (and the
/// scan saw it): refused with the reason that the files are not where
/// Printdex put them, and nothing on disk changes.
#[tokio::test]
async fn undo_after_external_rename_is_refused() {
    let f = Fx::new(&["Hook/hook.stl", "Storage/Bin/bin.stl"]).await;
    let id = f.find("Hook").await;
    let op = f
        .engine
        .move_models(&[id], f.lib.id, "Storage")
        .await
        .unwrap();
    fs::rename(
        f.root().join("Storage/Hook"),
        f.root().join("Storage/Wall hook"),
    )
    .unwrap();
    // Before and after the scan notices.
    for rescan in [false, true] {
        if rescan {
            f.engine
                .rescan(f.lib.id, CancellationToken::new())
                .await
                .unwrap();
        }
        let r = f.engine.undo(op.id).await;
        assert!(matches!(r, Err(CoreError::ModelNotFound)), "{r:?}");
        assert!(!f.root().join("Hook").exists());
        assert_eq!(
            fs::read_to_string(f.root().join("Storage/Wall hook/hook.stl")).unwrap(),
            "Hook/hook.stl"
        );
    }
    assert_eq!(
        f.engine.model(id).await.unwrap().unwrap().rel_path,
        "Storage/Wall hook"
    );
}

/// One source file and where the move puts it, relative to the library.
const MOVED: &[(&str, &str)] = &[
    ("A/one.stl", "C/one.stl"),
    ("A/.one.stl.printdex.toml", "C/.one.stl.printdex.toml"),
    ("A/Two/two.stl", "C/Two/two.stl"),
    ("A/Two/photo.jpg", "C/Two/photo.jpg"),
    ("A/Two/.printdex.toml", "C/Two/.printdex.toml"),
    ("B/three.stl", "C/three.stl"),
    ("B/.three.stl.printdex.toml", "C/.three.stl.printdex.toml"),
];
const UNMOVED: &str = "B/Keep/keep.stl";

/// Three models in two folders, with sidecars; the sidecar writer is off
/// so only the move touches the files.
async fn fault_fixture() -> (Fx, Vec<ModelId>) {
    let f = Fx::new(&[
        "A/one.stl",
        "A/Two/two.stl",
        "A/Two/photo.jpg",
        "B/three.stl",
        UNMOVED,
    ])
    .await;
    for (src, _) in MOVED {
        if src.contains("printdex.toml") {
            fs::write(f.root().join(src), src).unwrap();
        }
    }
    f.engine.set_sidecars(f.lib.id, false).await.unwrap();
    let ids = vec![
        f.find("one").await,
        f.find("Two").await,
        f.find("three").await,
    ];
    (f, ids)
}

fn files_under(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for e in fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let t = e.file_type().unwrap();
        if t.is_dir() {
            files_under(root, &e.path(), out);
        } else {
            let rel = e.path().strip_prefix(root).unwrap().to_owned();
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Every file exists exactly once, intact, at its source or its target;
/// nothing else is on disk; the database points where the files are.
async fn assert_nothing_lost(f: &Fx, ids: &[ModelId], what: &str) {
    let root = f.root();
    for (src, dst) in MOVED {
        let at: Vec<&str> = [*src, *dst]
            .into_iter()
            .filter(|p| root.join(p).exists())
            .collect();
        assert_eq!(at.len(), 1, "{what}: {src} is at {at:?}");
        assert_eq!(
            fs::read_to_string(root.join(at[0])).unwrap(),
            *src,
            "{what}: {} changed",
            at[0]
        );
    }
    let mut all = Vec::new();
    files_under(root, root, &mut all);
    for p in &all {
        assert!(
            p == UNMOVED || MOVED.iter().any(|(s, d)| p == s || p == d),
            "{what}: stray file {p}"
        );
    }
    let expect = [
        ("A/one.stl", "C/one.stl"),
        ("A/Two", "C/Two"),
        ("B/three.stl", "C/three.stl"),
    ];
    for (id, (src, dst)) in ids.iter().zip(expect) {
        let d = f.engine.model(*id).await.unwrap().unwrap();
        let on_disk = if root.join(dst).exists() { dst } else { src };
        assert_eq!(d.rel_path, on_disk, "{what}: database disagrees with disk");
        assert!(d.missing_since.is_none(), "{what}");
        for p in &d.parts {
            assert!(
                root.join(&p.rel_path).exists(),
                "{what}: part {}",
                p.rel_path
            );
        }
    }
}

/// Fails the n-th operation of each kind in turn, until a run makes no
/// fault fire. Same drive and across drives (every rename forced to report
/// another drive). With `rollback`, the move's last step always fails as
/// well, so the faults also land in the rollback (where removes happen).
/// With `crash`, the n-th operation panics instead: the work stops dead,
/// nothing is rolled back and the journal entry stays Planned, as after a
/// real crash; `reconcile_ops` must then make disk and database agree.
#[tokio::test]
async fn fault_at_every_step_loses_nothing() {
    let ops = [Op::Write, Op::Rename, Op::Copy, Op::Remove, Op::Trash];
    let mut runs = 0;
    for crash in [false, true] {
        for rollback in [false, true] {
            for cross in [false, true] {
                for op in ops {
                    for n in 0.. {
                        assert!(n < 200, "{op:?} never ran out of faults");
                        runs += 1;
                        let what =
                            format!("crash={crash} rollback={rollback} cross={cross} {op:?} #{n}");
                        let (f, ids) = fault_fixture().await;
                        // Armed first, so it wins over the forced
                        // cross-device error.
                        let action = if crash {
                            Action::Before(Box::new(|_| panic!("simulated crash")))
                        } else {
                            Action::Fail(ErrorKind::Other)
                        };
                        let fault = faults::arm(f.root(), op, n, action);
                        // The last step moves three's sidecar: the 5th
                        // rename, or across drives the 7th file copied.
                        let last = rollback.then(|| {
                            let (op, nth) = if cross {
                                (Op::Copy, 6)
                            } else {
                                (Op::Rename, 4)
                            };
                            faults::arm(f.root(), op, nth, Action::Fail(ErrorKind::Other))
                        });
                        let forced = if cross { cross_drive(&f) } else { Vec::new() };
                        let r = f.engine.move_models(&ids, f.lib.id, "C").await;
                        let fired = fault.fired();
                        drop(forced);
                        drop(last);
                        drop(fault);
                        if fired == 0 {
                            assert_nothing_lost(&f, &ids, &what).await;
                            assert_eq!(r.is_ok(), !rollback, "{what}: {r:?}");
                            for (src, dst) in MOVED {
                                let p = if rollback { src } else { dst };
                                assert!(f.root().join(p).exists(), "{what}: {p}");
                            }
                            break;
                        }
                        assert!(r.is_err(), "{what}: a fault fired but the move succeeded");
                        if crash {
                            assert_eq!(last_op_state(&f).await, OpState::Planned, "{what}");
                        } else {
                            assert_nothing_lost(&f, &ids, &what).await;
                        }
                        f.engine.reconcile_ops().await.unwrap();
                        assert_nothing_lost(&f, &ids, &format!("{what} reconciled")).await;
                    }
                }
            }
        }
    }
    assert!(runs > 80, "only {runs} runs");
}

/// A rollback whose removal of the copy stops halfway keeps the complete
/// copy it just put back, never the broken one.
#[tokio::test]
async fn partly_failed_remove_keeps_the_complete_copy() {
    let f = Fx::new(&[
        "Hook/a.stl",
        "Hook/b.stl",
        "Vase/vase.stl",
        "Storage/Bin/bin.stl",
    ])
    .await;
    let hook = f.find("Hook").await;
    let vase = f.find("Vase").await;
    // Hook's two files are copies 0 and 1; Vase's copy fails.
    let _vase = faults::arm(f.root(), Op::Copy, 2, Action::Fail(ErrorKind::StorageFull));
    // Removing Hook's copy takes one file and then fails.
    let _half = faults::arm(
        f.root(),
        Op::Remove,
        0,
        Action::Instead(Box::new(|p| {
            fs::remove_file(p.join("a.stl"))?;
            Err(std::io::Error::other("stopped halfway"))
        })),
    );
    let _cross = cross_drive(&f);
    let r = f
        .engine
        .move_models(&[hook, vase], f.lib.id, "Storage")
        .await;
    assert!(r.is_err());
    for p in ["Hook/a.stl", "Hook/b.stl", "Vase/vase.stl"] {
        assert_eq!(fs::read_to_string(f.root().join(p)).unwrap(), p);
    }
    assert_eq!(
        f.engine.model(hook).await.unwrap().unwrap().rel_path,
        "Hook"
    );
    assert_eq!(last_op_state(&f).await, OpState::Failed);
}

/// Journals a one-step Copy move of folder model `Hook` to `Storage/Hook`
/// and leaves it Planned, as a crash would; returns the engine reopened.
async fn planned_copy(prepare: impl FnOnce(&Path)) -> (tempfile::TempDir, Engine, ModelId) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("lib");
    write(&root, "Hook/hook.stl", "hook");
    write(&root, "Other/x.stl", "x");
    let data = dir.path().join("data");
    let (id, lib) = {
        let engine = Engine::open(&data).await.unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        let id = engine
            .search("Hook", 10)
            .await
            .unwrap()
            .into_iter()
            .find(|h| h.name == "Hook")
            .unwrap()
            .id;
        (id, lib)
    };
    let db = Db::open(&data.join("printdex.db")).await.unwrap();
    db.insert_op(
        OpKind::Move,
        &[Step {
            model: id,
            kind: ModelKind::Folder,
            name: "Hook".into(),
            new_name: "Hook".into(),
            from_lib: lib.id,
            from_rel: "Hook".into(),
            to_lib: lib.id,
            to_rel: "Storage/Hook".into(),
            how: How::Copy,
        }],
    )
    .await
    .unwrap();
    drop(db);
    prepare(&root);
    let engine = Engine::open(&data).await.unwrap();
    (dir, engine, id)
}

#[tokio::test]
async fn reconcile_removes_a_partial_copy() {
    let (dir, engine, id) = planned_copy(|root| {
        write(root, "Storage/Hook/hook.stl", "ho");
    })
    .await;
    let root = dir.path().join("lib");
    assert!(!root.join("Storage/Hook").exists());
    assert_eq!(
        fs::read_to_string(root.join("Hook/hook.stl")).unwrap(),
        "hook"
    );
    assert_eq!(engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
}

#[tokio::test]
async fn reconcile_keeps_a_target_that_is_not_a_copy() {
    let (dir, engine, id) = planned_copy(|root| {
        write(root, "Storage/Hook/hook.stl", "someone else's");
    })
    .await;
    let root = dir.path().join("lib");
    assert_eq!(
        fs::read_to_string(root.join("Storage/Hook/hook.stl")).unwrap(),
        "someone else's"
    );
    assert_eq!(
        fs::read_to_string(root.join("Hook/hook.stl")).unwrap(),
        "hook"
    );
    assert_eq!(engine.model(id).await.unwrap().unwrap().rel_path, "Hook");
}

#[tokio::test]
async fn reconcile_never_removes_the_only_copy() {
    // The copy finished and the original went to the trash.
    let (dir, engine, id) = planned_copy(|root| {
        write(root, "Storage/Hook/hook.stl", "hook");
        fs::remove_dir_all(root.join("Hook")).unwrap();
    })
    .await;
    let root = dir.path().join("lib");
    assert_eq!(
        fs::read_to_string(root.join("Storage/Hook/hook.stl")).unwrap(),
        "hook"
    );
    assert_eq!(
        engine.model(id).await.unwrap().unwrap().rel_path,
        "Storage/Hook"
    );
}

/// Trashing a single file whose sidecar goes first fails halfway: the model
/// file is never the one left in the trash while the action reports a
/// failure, so disk and database still agree that the model is there.
#[tokio::test]
async fn partly_failed_trash_keeps_the_model() {
    let f = Fx::new(&["Misc/hook.stl", "Misc/Sub/x.stl"]).await;
    let hook = f.find("hook").await;
    f.engine.set_sidecars(f.lib.id, false).await.unwrap();
    let side = f.root().join("Misc/.hook.stl.printdex.toml");
    fs::write(&side, "notes").unwrap();
    let bin = f.dir.path().join(".test-trash");
    let _to_bin = {
        let bin = bin.clone();
        faults::arm(
            f.root(),
            Op::Trash,
            0,
            Action::Instead(Box::new(move |p| {
                fs::create_dir_all(&bin)?;
                fs::rename(p, bin.join("0"))
            })),
        )
    };
    let _fail = faults::arm(f.root(), Op::Trash, 1, Action::Fail(ErrorKind::Other));
    assert!(f.engine.trash_models(&[hook]).await.is_err());
    assert_eq!(
        fs::read_to_string(f.root().join("Misc/hook.stl")).unwrap(),
        "Misc/hook.stl"
    );
    let d = f.engine.model(hook).await.unwrap().unwrap();
    assert!(d.missing_since.is_none());
    assert_eq!(last_op_state(&f).await, OpState::Failed);
}

/// Sends what the fixture's actions trash to a `.test-trash` folder
/// instead of the system trash.
fn test_trash(f: &Fx) -> Vec<FaultGuard> {
    let bin = f.dir.path().join(".test-trash");
    (0..64)
        .map(|k| {
            let bin = bin.clone();
            faults::arm(
                f.root(),
                Op::Trash,
                k,
                Action::Instead(Box::new(move |p| {
                    fs::create_dir_all(&bin)?;
                    fs::rename(p, bin.join(k.to_string()))
                })),
            )
        })
        .collect()
}

/// What [`fault_at_every_step_of_rename_undo_and_trash`] runs.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Action1 {
    Rename,
    Undo,
    Trash,
}

/// A single-file model `A/one.stl` with its sidecar, sidecar writer off.
async fn one_file_fixture() -> (Fx, ModelId) {
    let f = Fx::new(&["A/one.stl", "A/Sub/x.stl"]).await;
    fs::write(f.root().join("A/.one.stl.printdex.toml"), "side").unwrap();
    f.engine.set_sidecars(f.lib.id, false).await.unwrap();
    let id = f.find("one").await;
    (f, id)
}

/// The model file and its sidecar are each at most once, intact, at the
/// old or the new name, side by side, and otherwise in the test trash;
/// nothing else appeared; the database points where the file is.
async fn assert_one_file_kept(f: &Fx, id: ModelId, what: &str) {
    let root = f.root();
    let bin = f.dir.path().join(".test-trash");
    let mut binned = Vec::new();
    if bin.exists() {
        for e in fs::read_dir(&bin).unwrap() {
            binned.push(fs::read_to_string(e.unwrap().path()).unwrap());
        }
    }
    let mut sides = Vec::new();
    for (name, body) in [("one.stl", "A/one.stl"), (".one.stl.printdex.toml", "side")] {
        let at: Vec<String> = [name.to_owned(), name.replace("one", "uno")]
            .into_iter()
            .filter(|n| root.join("A").join(n).exists())
            .collect();
        // Across drives the original of a copy goes to the trash as well.
        let in_bin = binned.iter().filter(|b| *b == body).count();
        assert!(
            at.len() <= 1 && at.len() + in_bin >= 1,
            "{what}: {name} at {at:?}, {in_bin} in the trash"
        );
        if let Some(n) = at.first() {
            assert_eq!(
                fs::read_to_string(root.join("A").join(n)).unwrap(),
                body,
                "{what}"
            );
        }
        sides.push(at.first().map(|n| n.contains("uno")));
    }
    let mut all = Vec::new();
    files_under(root, root, &mut all);
    for p in &all {
        assert!(
            [
                "A/one.stl",
                "A/uno.stl",
                "A/.one.stl.printdex.toml",
                "A/.uno.stl.printdex.toml",
                "A/Sub/x.stl"
            ]
            .contains(&p.as_str()),
            "{what}: stray file {p}"
        );
    }
    match sides[0] {
        Some(renamed) => {
            if let Some(side) = sides[1] {
                assert_eq!(side, renamed, "{what}: sidecar apart from its model");
            }
            let d = f.engine.model(id).await.unwrap().unwrap();
            let rel = if renamed { "A/uno.stl" } else { "A/one.stl" };
            assert_eq!(d.rel_path, rel, "{what}: database disagrees with disk");
            assert!(d.missing_since.is_none(), "{what}");
        }
        // Trashed: the database may still show it until the next scan,
        // but never at another path.
        None => {
            if let Some(d) = f.engine.model(id).await.unwrap() {
                assert_eq!(d.rel_path, "A/one.stl", "{what}");
            }
        }
    }
}

/// Like [`fault_at_every_step_loses_nothing`], for renaming a model,
/// undoing that rename and trashing a model: fails (or crashes at) the n-th
/// operation of each kind in turn until none fires, same drive and across
/// drives, and checks nothing is lost before and after `reconcile_ops`.
#[tokio::test]
async fn fault_at_every_step_of_rename_undo_and_trash() {
    let ops = [Op::Write, Op::Rename, Op::Copy, Op::Remove, Op::Trash];
    let mut runs = 0;
    for action in [Action1::Rename, Action1::Undo, Action1::Trash] {
        for crash in [false, true] {
            for cross in [false, true] {
                for op in ops {
                    for n in 0.. {
                        assert!(n < 100, "{op:?} never ran out of faults");
                        runs += 1;
                        let what = format!("{action:?} crash={crash} cross={cross} {op:?} #{n}");
                        let (f, id) = one_file_fixture().await;
                        let undo = match action {
                            Action1::Undo => {
                                Some(f.engine.rename_model(id, "uno").await.unwrap().id)
                            }
                            _ => None,
                        };
                        let fa = if crash {
                            Action::Before(Box::new(|_| panic!("simulated crash")))
                        } else {
                            Action::Fail(ErrorKind::Other)
                        };
                        // Armed after the rename an undo needs, so the n-th
                        // counts only this action's operations, and before
                        // the forced cross-drive errors, so it wins.
                        let fault = faults::arm(f.root(), op, n, fa);
                        let forced = if cross {
                            cross_drive(&f)
                        } else {
                            test_trash(&f)
                        };
                        let r = match (action, undo) {
                            (Action1::Rename, _) => f.engine.rename_model(id, "uno").await,
                            (Action1::Undo, Some(op)) => f.engine.undo(op).await,
                            _ => f.engine.trash_models(&[id]).await,
                        };
                        let fired = fault.fired();
                        drop(fault);
                        if fired == 0 {
                            assert!(r.is_ok(), "{what}: {r:?}");
                            drop(forced);
                            assert_one_file_kept(&f, id, &what).await;
                            break;
                        }
                        assert!(r.is_err(), "{what}: a fault fired but it succeeded");
                        if crash {
                            assert_eq!(last_op_state(&f).await, OpState::Planned, "{what}");
                        } else {
                            assert_one_file_kept(&f, id, &what).await;
                        }
                        f.engine.reconcile_ops().await.unwrap();
                        drop(forced);
                        assert_one_file_kept(&f, id, &format!("{what} reconciled")).await;
                    }
                }
            }
        }
    }
    assert!(runs > 30, "only {runs} runs");
}
