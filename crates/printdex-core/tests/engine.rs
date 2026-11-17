//! End-to-end engine tests through the public API.

use std::fs;
use std::path::{Path, PathBuf};

use printdex_core::{
    CoreError, Engine, EngineEvent, LibraryStatus, PrintableFormat, SearchFilter, SortOrder,
};
use tokio_util::sync::CancellationToken;

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
}

impl Fx {
    async fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(&dir.path().join("data")).await.unwrap();
        Fx { dir, engine }
    }

    fn lib(&self, files: &[&str]) -> PathBuf {
        let root = self.dir.path().join("lib");
        for f in files {
            touch(&root, f);
        }
        fs::create_dir_all(&root).unwrap();
        root
    }
}

fn touch(root: &Path, rel: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, rel.as_bytes()).unwrap();
}

fn go() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn add_scan_search_roundtrip() {
    let f = Fx::new().await;
    let root = f.lib(&["Red Dragon/a.stl", "Orc/orc.stl"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    assert_eq!(lib.display_name, "lib");
    f.engine.rescan(lib.id, go()).await.unwrap();
    let hits = f.engine.search("dragon", 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Red Dragon");
    assert_eq!(hits[0].part_count, 1);
}

#[tokio::test]
async fn finds_models_by_non_ascii_name() {
    let f = Fx::new().await;
    let root = f.lib(&["龍/x.stl", "Drachen_Ü/y.3MF"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    assert_eq!(f.engine.search("龍", 10).await.unwrap().len(), 1);
    assert_eq!(f.engine.search("drachen", 10).await.unwrap().len(), 1);
}

#[tokio::test]
async fn search_with_syntax_chars_does_not_error() {
    let f = Fx::new().await;
    let root = f.lib(&["Red Dragon/a.stl"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    for q in [
        "\"",
        "*",
        "(",
        "-",
        "AND",
        "NEAR(",
        "a:b",
        "dragon (v2)",
        "^",
        "+",
        "{x}",
    ] {
        assert!(f.engine.search(q, 10).await.is_ok(), "query {q:?} failed");
    }
}

#[tokio::test]
async fn rescan_of_offline_library_marks_nothing_missing() {
    let f = Fx::new().await;
    let root = f.lib(&["Red Dragon/a.stl", "Orc/orc.stl"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    let away = f.dir.path().join("away");
    fs::rename(&root, &away).unwrap();
    assert!(matches!(
        f.engine.rescan(lib.id, go()).await,
        Err(CoreError::LibraryOffline(_))
    ));
    assert_eq!(f.engine.stats().await.unwrap().missing_models, 0);
    fs::rename(&away, &root).unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    let stats = f.engine.stats().await.unwrap();
    assert_eq!((stats.models, stats.missing_models), (2, 0));
}

#[tokio::test]
async fn rejects_overlapping_libraries() {
    let f = Fx::new().await;
    let x = f.dir.path().join("x");
    let lib = x.join("lib");
    fs::create_dir_all(lib.join("sub")).unwrap();
    f.engine.add_library(&lib, None).await.unwrap();
    for p in [lib.join("sub"), x.clone(), lib.join(".")] {
        assert!(
            matches!(
                f.engine.add_library(&p, None).await,
                Err(CoreError::LibraryOverlaps { .. })
            ),
            "{p:?} was accepted"
        );
    }
    assert_eq!(f.engine.libraries().await.unwrap().len(), 1);
    assert!(
        f.engine
            .library_by_path(&lib.join("."))
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn rejects_missing_path() {
    let f = Fx::new().await;
    let r = f.engine.add_library(&f.dir.path().join("nope"), None).await;
    assert!(matches!(r, Err(CoreError::LibraryPathInvalid(_))));
    let file = f.dir.path().join("file.stl");
    fs::write(&file, b"x").unwrap();
    let r = f.engine.add_library(&file, None).await;
    assert!(matches!(r, Err(CoreError::LibraryPathInvalid(_))));
}

#[tokio::test]
async fn concurrent_rescan_is_rejected() {
    let f = Fx::new().await;
    let names: Vec<String> = (0..2_000)
        .map(|i| format!("M{}/p{i}.stl", i % 200))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let root = f.lib(&refs);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    let mut rx = f.engine.subscribe();
    let first = tokio::spawn({
        let engine = f.engine.clone();
        async move { engine.rescan(lib.id, go()).await }
    });
    // Wait until the first scan is running.
    loop {
        if let EngineEvent::ScanStarted { .. } = rx.recv().await.unwrap() {
            break;
        }
    }
    assert!(matches!(
        f.engine.rescan(lib.id, go()).await,
        Err(CoreError::AlreadyScanning(_))
    ));
    first.await.unwrap().unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
}

#[tokio::test]
async fn events_reach_subscribers() {
    let f = Fx::new().await;
    let root = f.lib(&["Orc/orc.stl"]);
    let mut rx = f.engine.subscribe();
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    let mut seen = Vec::new();
    while let Ok(e) = rx.try_recv() {
        seen.push(match e {
            EngineEvent::LibraryAdded(_) => "added",
            EngineEvent::ScanStarted { .. } => "started",
            EngineEvent::ScanProgress { .. } => continue,
            EngineEvent::ScanFinished { .. } => "finished",
            EngineEvent::ScanFailed { .. } => "failed",
            EngineEvent::LibraryRemoved(_) => "removed",
            EngineEvent::LibraryRenamed(_) => "renamed",
        });
    }
    assert_eq!(seen, ["added", "started", "finished"]);
}

#[tokio::test]
async fn data_persists_across_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let root = dir.path().join("lib");
    touch(&root, "Red Dragon/a.stl");
    {
        let engine = Engine::open(&data).await.unwrap();
        let lib = engine
            .add_library(&root, Some("Minis".into()))
            .await
            .unwrap();
        engine.rescan(lib.id, go()).await.unwrap();
    }
    let engine = Engine::open(&data).await.unwrap();
    let libs = engine.libraries().await.unwrap();
    assert_eq!(libs.len(), 1);
    assert_eq!(libs[0].display_name, "Minis");
    assert_eq!(engine.search("dragon", 10).await.unwrap().len(), 1);
}

#[test]
fn default_data_dir_is_available() {
    assert!(Engine::default_data_dir().is_some());
}

#[tokio::test]
async fn search_in_scopes_to_library() {
    let f = Fx::new().await;
    let a = f.dir.path().join("a");
    let b = f.dir.path().join("b");
    touch(&a, "Dragon/x.stl");
    touch(&a, "Orc/y.stl");
    touch(&b, "Dragon/z.stl");
    let la = f.engine.add_library(&a, None).await.unwrap();
    let lb = f.engine.add_library(&b, None).await.unwrap();
    f.engine.rescan(la.id, go()).await.unwrap();
    f.engine.rescan(lb.id, go()).await.unwrap();
    let none = SearchFilter::default();
    let hits = f
        .engine
        .search_in(Some(lb.id), "dragon", &none, SortOrder::Name, 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].library_id, lb.id);
    assert_eq!(f.engine.count(None, "dragon", &none).await.unwrap(), 2);
    assert_eq!(f.engine.count(Some(la.id), "", &none).await.unwrap(), 2);
    let counts = f.engine.library_counts().await.unwrap();
    assert_eq!((counts[&la.id], counts[&lb.id]), (2, 1));
}

#[tokio::test]
async fn model_detail_through_engine() {
    let f = Fx::new().await;
    let root = f.lib(&["Bin/a.stl", "Bin/b.3mf", "Bin/photo.jpg"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    let hit = &f.engine.search("bin", 5).await.unwrap()[0];
    let detail = f.engine.model(hit.id).await.unwrap().unwrap();
    assert_eq!(detail.parts.len(), 2);
    assert_eq!(detail.parts[1].format, PrintableFormat::ThreeMf);
    assert_eq!(detail.assets.len(), 1);
}

#[tokio::test]
async fn remove_library_keeps_files() {
    let f = Fx::new().await;
    let root = f.lib(&["Bin/a.stl"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    f.engine.rescan(lib.id, go()).await.unwrap();
    let mut events = f.engine.subscribe();
    f.engine.remove_library(lib.id).await.unwrap();
    assert!(root.join("Bin/a.stl").is_file());
    assert!(f.engine.libraries().await.unwrap().is_empty());
    assert_eq!(f.engine.stats().await.unwrap().models, 0);
    assert!(
        matches!(events.recv().await.unwrap(), EngineEvent::LibraryRemoved(id) if id == lib.id)
    );
    assert!(matches!(
        f.engine.remove_library(lib.id).await,
        Err(CoreError::LibraryNotFound(_))
    ));
}

#[tokio::test]
async fn rename_library_changes_display_name() {
    let f = Fx::new().await;
    let lib = f.engine.add_library(&f.lib(&[]), None).await.unwrap();
    let renamed = f
        .engine
        .rename_library(lib.id, "  Workshop SSD ")
        .await
        .unwrap();
    assert_eq!(renamed.display_name, "Workshop SSD");
    assert!(matches!(
        f.engine.rename_library(lib.id, "   ").await,
        Err(CoreError::LibraryNameInvalid)
    ));
}

#[tokio::test]
async fn library_status_offline_when_root_missing() {
    let f = Fx::new().await;
    let root = f.lib(&["Bin/a.stl"]);
    let lib = f.engine.add_library(&root, None).await.unwrap();
    assert_eq!(
        f.engine.library_status(lib.id).await.unwrap(),
        LibraryStatus::Online
    );
    fs::rename(&root, f.dir.path().join("unplugged")).unwrap();
    assert_eq!(
        f.engine.library_status(lib.id).await.unwrap(),
        LibraryStatus::Offline
    );
}
