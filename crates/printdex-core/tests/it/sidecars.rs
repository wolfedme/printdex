//! `.printdex.toml` sidecars end to end: writing, reading back, restoring.

use std::fs;
use std::path::{Path, PathBuf};

use printdex_core::sidecar::SidecarFile;
use printdex_core::{
    AttemptResult, Engine, EngineEvent, Library, Material, ModelId, NewAttempt, PrintSettings,
    Recipe,
};
use tokio_util::sync::CancellationToken;

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    lib: Library,
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

impl Fx {
    async fn new(files: &[&str]) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("lib");
        fs::create_dir_all(&root).unwrap();
        for f in files {
            touch(&root, f);
        }
        let engine = Engine::open(&dir.path().join("data")).await.unwrap();
        let lib = engine.add_library(&root, None).await.unwrap();
        engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
        Fx { dir, engine, lib }
    }

    fn root(&self) -> PathBuf {
        self.lib.path.clone()
    }

    async fn reopen(self) -> Fx {
        let Fx { dir, engine, lib } = self;
        drop(engine);
        let engine = Engine::open(&dir.path().join("data")).await.unwrap();
        Fx { dir, engine, lib }
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

    async fn rescan(&self) -> printdex_core::ScanSummary {
        self.engine
            .rescan(self.lib.id, CancellationToken::new())
            .await
            .unwrap()
    }

    fn read(&self, rel: &str) -> SidecarFile {
        SidecarFile::parse(&fs::read_to_string(self.root().join(rel)).unwrap()).unwrap()
    }
}

fn attempt(result: AttemptResult) -> NewAttempt {
    NewAttempt {
        printed_at: 1_763_000_000_000,
        result,
        settings: PrintSettings {
            material: Some(Material::Petg),
            layer_mm: Some(0.2),
            ..PrintSettings::default()
        },
        duration_min: Some(90),
        notes: "Corners lifted".into(),
        parts: vec![],
    }
}

#[tokio::test]
async fn writes_folder_and_file_sidecars() {
    let f = Fx::new(&["Bin/a.stl", "Bin/b.stl", "Misc/hook.stl", "Misc/Sub/x.stl"]).await;
    let bin = f.model("Bin").await;
    let hook = f.model("hook").await;
    f.engine.add_tag(bin, "Gridfinity").await.unwrap();
    f.engine.set_notes(hook, "Print flat.").await.unwrap();
    // Nothing is written before the debounce or a flush.
    assert!(!f.root().join("Bin/.printdex.toml").exists());
    f.engine.flush_sidecars().await;
    let s = f.read("Bin/.printdex.toml");
    assert_eq!(s.tags, ["Gridfinity"]);
    assert_eq!(s.model_id, bin.to_string());
    assert_eq!(f.read("Misc/.hook.stl.printdex.toml").notes, "Print flat.");
    // Models without data get no file.
    assert!(!f.root().join("Misc/Sub/.printdex.toml").exists());
}

#[tokio::test]
async fn writes_after_debounce() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "x").await.unwrap();
    tokio::time::sleep(printdex_core::sidecar::DEBOUNCE * 2).await;
    assert_eq!(f.read("Bin/.printdex.toml").tags, ["x"]);
}

#[tokio::test]
async fn sidecar_is_not_an_asset_and_not_reimported() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "x").await.unwrap();
    f.engine.flush_sidecars().await;
    let summary = f.rescan().await;
    assert_eq!(summary.sidecars_imported, 0, "own write is not imported");
    let detail = f.engine.model(bin).await.unwrap().unwrap();
    assert!(detail.assets.is_empty());
}

#[tokio::test]
async fn disabled_library_writes_nothing() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    f.engine.set_sidecars(f.lib.id, false).await.unwrap();
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "x").await.unwrap();
    f.engine.flush_sidecars().await;
    assert!(!f.root().join("Bin/.printdex.toml").exists());
    // Turning it on writes models with data.
    f.engine.set_sidecars(f.lib.id, true).await.unwrap();
    f.engine.flush_sidecars().await;
    assert_eq!(f.read("Bin/.printdex.toml").tags, ["x"]);
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_folder_falls_back_to_db() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    let dir = f.root().join("Bin");
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores permissions; only check when the folder really is read-only.
    let readonly = fs::write(dir.join("probe"), b"x").is_err();
    f.engine.add_tag(bin, "kept").await.unwrap();
    f.engine.flush_sidecars().await;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    if readonly {
        assert!(!dir.join(".printdex.toml").exists());
    }
    let data = f.engine.model_data(bin).await.unwrap().unwrap();
    assert_eq!(data.tags, ["kept"]);
}

#[tokio::test]
async fn restore_from_sidecars_after_db_loss() {
    let f = Fx::new(&["Bin/a.stl", "Misc/hook.stl", "Misc/Sub/x.stl"]).await;
    let bin = f.model("Bin").await;
    let hook = f.model("hook").await;
    f.engine.add_tag(bin, "gridfinity").await.unwrap();
    f.engine.set_notes(bin, "Use the 3MF.").await.unwrap();
    f.engine
        .set_source(bin, Some("https://www.printables.com/model/1"))
        .await
        .unwrap();
    let gifts = f.engine.create_collection("Gifts").await.unwrap();
    f.engine.add_to_collection(gifts.id, hook).await.unwrap();
    let recipe = Recipe {
        settings: PrintSettings {
            material: Some(Material::Petg),
            bed_temp: Some(85),
            ..PrintSettings::default()
        },
        ..Recipe::default()
    };
    f.engine.set_recipe(bin, Some(&recipe)).await.unwrap();
    f.engine
        .log_attempt(bin, &attempt(AttemptResult::Failed))
        .await
        .unwrap();
    f.engine.flush_sidecars().await;
    let before = f.engine.model_data(bin).await.unwrap().unwrap();

    // Lose the database, keep the library.
    let root = f.root();
    let fresh_dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(&fresh_dir.path().join("data")).await.unwrap();
    let lib = engine.add_library(&root, None).await.unwrap();
    let summary = engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(summary.sidecars_imported, 2);

    let find = |name: &'static str| {
        let engine = engine.clone();
        async move {
            engine
                .search(name, 10)
                .await
                .unwrap()
                .into_iter()
                .find(|h| h.name == name)
                .unwrap()
                .id
        }
    };
    let bin2 = find("Bin").await;
    let after = engine.model_data(bin2).await.unwrap().unwrap();
    assert_eq!(after.tags, before.tags);
    assert_eq!(after.notes, before.notes);
    assert_eq!(after.source, before.source);
    assert_eq!(after.recipe.as_ref().unwrap().settings, recipe.settings);
    assert_eq!(after.attempts.len(), 1);
    assert_eq!(after.attempts[0].id, before.attempts[0].id);
    assert_eq!(after.attempts[0].notes, "Corners lifted");
    let hook2 = find("hook").await;
    let names: Vec<_> = engine
        .model_data(hook2)
        .await
        .unwrap()
        .unwrap()
        .collections
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["Gifts"]);
    // Restored data is searchable.
    assert_eq!(engine.search("gridfinity", 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn edited_sidecar_wins_per_record() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "old").await.unwrap();
    let a = f
        .engine
        .log_attempt(bin, &attempt(AttemptResult::Failed))
        .await
        .unwrap();
    f.engine.flush_sidecars().await;

    // A hand edit: new tags, and the attempt's notes changed with a newer
    // updated_at.
    let mut s = f.read("Bin/.printdex.toml");
    s.tags = vec!["new".into()];
    s.updated_at += 1000;
    s.attempts[0].notes = "Edited by hand".into();
    s.attempts[0].updated_at += 1000;
    // mtime must differ from the recorded one.
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(f.root().join("Bin/.printdex.toml"), s.to_toml().unwrap()).unwrap();

    let summary = f.rescan().await;
    assert_eq!(summary.sidecars_imported, 1);
    let data = f.engine.model_data(bin).await.unwrap().unwrap();
    assert_eq!(data.tags, ["new"]);
    assert_eq!(data.attempts[0].id, a);
    assert_eq!(data.attempts[0].notes, "Edited by hand");
}

#[tokio::test]
async fn older_sidecar_does_not_override() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "current").await.unwrap();
    f.engine.flush_sidecars().await;
    let mut s = f.read("Bin/.printdex.toml");
    s.tags = vec!["stale".into()];
    s.updated_at -= 1000;
    std::thread::sleep(std::time::Duration::from_millis(20));
    fs::write(f.root().join("Bin/.printdex.toml"), s.to_toml().unwrap()).unwrap();
    f.rescan().await;
    assert_eq!(
        f.engine.model_data(bin).await.unwrap().unwrap().tags,
        ["current"]
    );
    // The stale file is rewritten with the newer data.
    f.engine.flush_sidecars().await;
    assert_eq!(f.read("Bin/.printdex.toml").tags, ["current"]);
}

#[tokio::test]
async fn bad_sidecar_is_skipped() {
    let f = Fx::new(&["Bin/a.stl", "Box/b.stl", "Big/c.stl"]).await;
    fs::write(f.root().join("Bin/.printdex.toml"), "garbage = [").unwrap();
    fs::write(
        f.root().join("Box/.printdex.toml"),
        "schema = 99\ntags = [\"x\"]",
    )
    .unwrap();
    let big = format!("schema = 1\nnotes = \"{}\"\n", "x".repeat(2 * 1024 * 1024));
    fs::write(f.root().join("Big/.printdex.toml"), big).unwrap();
    let summary = f.rescan().await;
    assert_eq!(summary.sidecars_imported, 0);
    let bin = f.model("Bin").await;
    assert!(
        f.engine
            .model_data(bin)
            .await
            .unwrap()
            .unwrap()
            .tags
            .is_empty()
    );
    // The broken file is left alone.
    assert_eq!(
        fs::read_to_string(f.root().join("Bin/.printdex.toml")).unwrap(),
        "garbage = ["
    );
}

#[tokio::test]
async fn foreign_install_warns() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let other = printdex_core::ids::Uuid::now_v7().to_string();
    fs::write(
        f.root().join("Bin/.printdex.toml"),
        format!("schema = 1\nwriter = \"{other}\"\nupdated_at = 5\ntags = [\"theirs\"]\n"),
    )
    .unwrap();
    let mut rx = f.engine.subscribe();
    f.rescan().await;
    let mut warned = 0;
    while let Ok(e) = rx.try_recv() {
        if matches!(e, EngineEvent::SidecarForeign { library } if library == f.lib.id) {
            warned += 1;
        }
    }
    assert_eq!(warned, 1);
    let bin = f.model("Bin").await;
    assert_eq!(
        f.engine.model_data(bin).await.unwrap().unwrap().tags,
        ["theirs"]
    );
}

#[tokio::test]
async fn edits_emit_model_data_changed() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    let mut rx = f.engine.subscribe();
    f.engine.add_tag(bin, "x").await.unwrap();
    f.engine.set_notes(bin, "n").await.unwrap();
    let mut seen = Vec::new();
    while let Ok(e) = rx.try_recv() {
        match e {
            EngineEvent::ModelDataChanged(id) => seen.push(format!("data {}", id == bin)),
            EngineEvent::OrganizeChanged => seen.push("lists".into()),
            _ => {}
        }
    }
    assert_eq!(seen, ["data true", "lists", "data true"]);
}

#[tokio::test]
async fn data_survives_reopen() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "x").await.unwrap();
    let f = f.reopen().await;
    assert_eq!(f.engine.model_data(bin).await.unwrap().unwrap().tags, ["x"]);
}

async fn number_of(f: &Fx, name: &str) -> Option<u32> {
    f.engine
        .search(name, 10)
        .await
        .unwrap()
        .into_iter()
        .find(|h| h.name == name)
        .unwrap()
        .number
}

#[tokio::test]
async fn catalogue_numbers_are_never_reused() {
    let f = Fx::new(&["A/a.stl", "B/b.stl"]).await;
    assert_eq!(number_of(&f, "A").await, Some(1));
    assert_eq!(number_of(&f, "B").await, Some(2));
    fs::remove_dir_all(f.root().join("B")).unwrap();
    touch(&f.root(), "C/c.stl");
    f.rescan().await;
    assert_eq!(
        number_of(&f, "C").await,
        Some(3),
        "B's number stays retired"
    );
    // B comes back with its old number.
    touch(&f.root(), "B/b.stl");
    f.rescan().await;
    assert_eq!(number_of(&f, "B").await, Some(2));
}

#[tokio::test]
async fn search_finds_catalogue_numbers() {
    let f = Fx::new(&["A/a.stl", "B/b.stl", "Part 2/x.stl"]).await;
    let names = |hits: Vec<printdex_core::ModelHit>| -> Vec<String> {
        hits.into_iter().map(|h| h.name).collect()
    };
    // "№ 2" is only the number; plain "2" also matches names, number first.
    assert_eq!(names(f.engine.search("№ 2", 10).await.unwrap()), ["B"]);
    assert_eq!(names(f.engine.search("#0002", 10).await.unwrap()), ["B"]);
    assert_eq!(
        names(f.engine.search("2", 10).await.unwrap()),
        ["B", "Part 2"]
    );
}

#[tokio::test]
async fn sidecar_brings_number_back_after_reinstall() {
    let f = Fx::new(&["A/a.stl", "B/b.stl", "C/c.stl"]).await;
    let c = f.model("C").await;
    f.engine.add_tag(c, "keep").await.unwrap();
    f.engine.flush_sidecars().await;
    assert_eq!(f.read("C/.printdex.toml").number, Some(3));
    // A fresh install, with a new model that sorts first.
    touch(&f.root(), "0 New/n.stl");
    let engine = Engine::open(&f.dir.path().join("data2")).await.unwrap();
    let lib = engine.add_library(&f.root(), None).await.unwrap();
    engine
        .rescan(lib.id, CancellationToken::new())
        .await
        .unwrap();
    let g = Fx {
        dir: tempfile::tempdir().unwrap(),
        engine,
        lib,
    };
    assert_eq!(number_of(&g, "C").await, Some(3));
    // The others continue after the highest known number: lower ones may
    // have belonged to models that are gone, so they are not handed out.
    let mut rest = vec![
        number_of(&g, "0 New").await.unwrap(),
        number_of(&g, "A").await.unwrap(),
        number_of(&g, "B").await.unwrap(),
    ];
    rest.sort_unstable();
    assert_eq!(rest, [4, 5, 6]);
}

#[tokio::test]
async fn sidecar_write_failure_keeps_old_sidecar() {
    use printdex_core::fsx::faults::{Action, Op, arm};
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "first").await.unwrap();
    f.engine.flush_sidecars().await;
    let path = f.root().join("Bin/.printdex.toml");
    let before = fs::read_to_string(&path).unwrap();

    let g = arm(
        &f.root(),
        Op::Rename,
        0,
        Action::Fail(std::io::ErrorKind::StorageFull),
    );
    f.engine.add_tag(bin, "second").await.unwrap();
    f.engine.flush_sidecars().await;
    assert_eq!(g.fired(), 1, "the sidecar write went through fsx");
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    let names: Vec<_> = fs::read_dir(f.root().join("Bin"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.contains("tmp")), "{names:?}");
}

#[tokio::test]
async fn scan_sweeps_stale_temp_next_to_sidecar() {
    const U: &str = "0190aaaabbbbccccddddeeeeffff0000";
    let f = Fx::new(&["Bin/a.stl", "Other/b.stl"]).await;
    let bin = f.model("Bin").await;
    f.engine.add_tag(bin, "x").await.unwrap();
    f.engine.flush_sidecars().await;
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(2 * 3600);
    let make = |rel: &str, aged: bool| {
        let p = f.root().join(rel);
        fs::write(&p, b"partial").unwrap();
        if aged {
            fs::File::options()
                .write(true)
                .open(&p)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        p
    };
    let stale = make(&format!("Bin/.printdex.toml.tmp-{U}"), true);
    let fresh = make(&format!("Bin/.fresh.toml.tmp-{U}"), false);
    // A folder without a sidecar is not visited.
    let elsewhere = make(&format!("Other/.x.tmp-{U}"), true);
    f.rescan().await;
    assert!(!stale.exists(), "stale temp next to a sidecar is swept");
    assert!(fresh.exists(), "a recent temp may still be in use");
    assert!(elsewhere.exists());
}

#[tokio::test]
async fn broken_sidecar_is_set_aside_before_rewrite() {
    let f = Fx::new(&["Bin/a.stl"]).await;
    let bin = f.model("Bin").await;
    let path = f.root().join("Bin/.printdex.toml");
    fs::write(&path, "garbage = [").unwrap();
    // A scan only reads: the broken file stays where it is.
    f.rescan().await;
    assert_eq!(fs::read_to_string(&path).unwrap(), "garbage = [");
    f.engine.add_tag(bin, "kept").await.unwrap();
    f.engine.flush_sidecars().await;
    assert_eq!(f.read("Bin/.printdex.toml").tags, ["kept"]);
    let broken: Vec<_> = fs::read_dir(f.root().join("Bin"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".printdex.toml.broken-")
        })
        .collect();
    assert_eq!(broken.len(), 1, "{broken:?}");
    assert_eq!(fs::read_to_string(&broken[0]).unwrap(), "garbage = [");
    // Neither a later scan nor the temp sweep takes the set-aside file, and
    // it is never read as a sidecar.
    f.rescan().await;
    assert!(broken[0].exists());
    assert!(!printdex_core::walk::is_sidecar_name(
        &broken[0].file_name().unwrap().to_string_lossy()
    ));
}
