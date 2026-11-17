//! Thumbnail service end to end: scan a library of real files, request
//! thumbnails, wait for the events.

use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use printdex_core::{Engine, EngineEvent, ModelId, ThumbFailure, ThumbTarget};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

struct Fx {
    dir: tempfile::TempDir,
    engine: Engine,
    events: broadcast::Receiver<EngineEvent>,
}

impl Fx {
    async fn new() -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open_with_cache(&dir.path().join("data"), &dir.path().join("cache"))
            .await
            .unwrap();
        let events = engine.subscribe();
        Fx {
            dir,
            engine,
            events,
        }
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("lib")
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let p = self.root().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, bytes).unwrap();
    }

    /// Scans the library and returns the model named `name`.
    async fn scan(&self) {
        let lib = match self.engine.library_by_path(&self.root()).await.unwrap() {
            Some(l) => l,
            None => self.engine.add_library(&self.root(), None).await.unwrap(),
        };
        self.engine
            .rescan(lib.id, CancellationToken::new())
            .await
            .unwrap();
    }

    async fn model(&self, name: &str) -> ModelId {
        let hits = self.engine.search("", 100).await.unwrap();
        hits.iter()
            .find(|h| h.name == name)
            .unwrap_or_else(|| panic!("no model {name}"))
            .id
    }

    /// Requests `target` and waits for its event.
    async fn thumb(&mut self, target: ThumbTarget) -> Result<PathBuf, ThumbFailure> {
        self.engine.thumbnails().request_visible([target]);
        let wait = async {
            loop {
                match self.events.recv().await.unwrap() {
                    EngineEvent::ThumbnailReady {
                        target: t, path, ..
                    } if t == target => {
                        return Ok(path);
                    }
                    EngineEvent::ThumbnailFailed { target: t, reason } if t == target => {
                        return Err(reason);
                    }
                    _ => {}
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(20), wait)
            .await
            .expect("thumbnail event")
    }
}

/// Binary STL of a `x` × `y` × `z` box.
fn cuboid_stl(x: f32, y: f32, z: f32) -> Vec<u8> {
    let c = |i: usize| {
        [
            if i & 1 == 0 { 0.0 } else { x },
            if i & 2 == 0 { 0.0 } else { y },
            if i & 4 == 0 { 0.0 } else { z },
        ]
    };
    let faces = [
        [0, 1, 3, 2],
        [4, 6, 7, 5],
        [0, 4, 5, 1],
        [2, 3, 7, 6],
        [0, 2, 6, 4],
        [1, 5, 7, 3],
    ];
    let mut out = vec![0u8; 80];
    out.extend(12u32.to_le_bytes());
    for f in faces {
        for tri in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
            out.extend([0u8; 12]);
            for v in tri {
                for k in c(v) {
                    out.extend(k.to_le_bytes());
                }
            }
            out.extend([0u8; 2]);
        }
    }
    out
}

fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([rgb[0], rgb[1], rgb[2], 255]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

fn threemf_with_thumbnail() -> Vec<u8> {
    let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default();
    w.start_file("Metadata/plate_1.png", opts).unwrap();
    w.write_all(&png(512, 512, [10, 200, 30])).unwrap();
    w.finish().unwrap().into_inner()
}

fn decode(path: &Path) -> image::RgbaImage {
    image::open(path).unwrap().to_rgba8()
}

fn file_name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

#[tokio::test]
async fn ready_event_carries_path() {
    let mut f = Fx::new().await;
    f.write("Box/box.stl", &cuboid_stl(10.0, 20.0, 5.0));
    f.scan().await;
    let id = f.model("Box").await;
    let path = f.thumb(ThumbTarget::Model(id)).await.unwrap();
    let img = decode(&path);
    assert_eq!(img.dimensions(), (384, 288));
    assert!(img.pixels().any(|p| p[3] == 255), "model drawn");
    assert_eq!(img.get_pixel(0, 0)[3], 0, "transparent background");
}

#[tokio::test]
async fn measurements_stored() {
    let mut f = Fx::new().await;
    f.write("Box/box.stl", &cuboid_stl(10.0, 20.0, 5.0));
    f.scan().await;
    let id = f.model("Box").await;
    let part = f.engine.model(id).await.unwrap().unwrap().parts[0].clone();
    assert_eq!(part.dims, None);
    f.thumb(ThumbTarget::Part(part.id)).await.unwrap();
    let part = f.engine.model(id).await.unwrap().unwrap().parts[0].clone();
    assert_eq!(part.dims, Some([10.0, 20.0, 5.0]));
    assert_eq!(part.triangles, Some(12));

    // A changed file hides the old measurements until measured again.
    f.write("Box/box.stl", &cuboid_stl(30.0, 20.0, 5.0));
    f.scan().await;
    let part = f.engine.model(id).await.unwrap().unwrap().parts[0].clone();
    assert_eq!(part.dims, None);
}

#[tokio::test]
async fn cover_prefers_embedded_then_image_then_render() {
    let mut f = Fx::new().await;
    f.write("A/a.stl", &cuboid_stl(1.0, 1.0, 1.0));
    f.write("A/b.3mf", &threemf_with_thumbnail());
    f.write("A/photo.png", &png(800, 600, [200, 0, 0]));
    f.write("B/b.stl", &cuboid_stl(1.0, 1.0, 1.0));
    f.write("B/photo.png", &png(800, 600, [200, 0, 0]));
    f.write("C/c.stl", &cuboid_stl(1.0, 1.0, 1.0));
    f.scan().await;

    let a = f
        .thumb(ThumbTarget::Model(f.model("A").await))
        .await
        .unwrap();
    assert!(file_name(&a).starts_with('e'), "{a:?}");
    let px = *decode(&a).get_pixel(10, 10);
    assert_eq!([px[0], px[1], px[2]], [10, 200, 30]);
    assert!(decode(&a).width() <= 384 && decode(&a).height() <= 288);

    let b = f
        .thumb(ThumbTarget::Model(f.model("B").await))
        .await
        .unwrap();
    assert!(file_name(&b).starts_with('i'), "{b:?}");
    assert_eq!(decode(&b).dimensions(), (384, 288));

    let c = f
        .thumb(ThumbTarget::Model(f.model("C").await))
        .await
        .unwrap();
    assert!(file_name(&c).starts_with('p'), "{c:?}");
}

#[tokio::test]
async fn cache_keyed_by_fingerprint() {
    let mut f = Fx::new().await;
    f.write("One/x.stl", &cuboid_stl(4.0, 4.0, 4.0));
    f.write("Two/y.stl", &cuboid_stl(4.0, 4.0, 4.0));
    f.write("Three/z.stl", &cuboid_stl(4.0, 9.0, 4.0));
    f.scan().await;
    let one = f
        .thumb(ThumbTarget::Model(f.model("One").await))
        .await
        .unwrap();
    let two = f
        .thumb(ThumbTarget::Model(f.model("Two").await))
        .await
        .unwrap();
    let three = f
        .thumb(ThumbTarget::Model(f.model("Three").await))
        .await
        .unwrap();
    assert_eq!(one, two, "same content, same thumbnail");
    assert_ne!(one, three);
}

#[tokio::test]
async fn failed_part_is_not_retried() {
    let mut f = Fx::new().await;
    f.write("Broken/b.stl", b"solid nope\n vertex 1 2\n");
    f.scan().await;
    let id = f.model("Broken").await;
    assert_eq!(
        f.thumb(ThumbTarget::Model(id)).await,
        Err(ThumbFailure::Invalid)
    );
    let markers: Vec<_> = walk(f.engine.thumbnails().cache_dir())
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "fail"))
        .collect();
    assert_eq!(markers.len(), 1);
    assert_eq!(
        f.thumb(ThumbTarget::Model(id)).await,
        Err(ThumbFailure::Invalid)
    );
}

#[tokio::test]
async fn deleted_cache_rerenders() {
    let mut f = Fx::new().await;
    f.write("Box/box.stl", &cuboid_stl(3.0, 3.0, 3.0));
    f.scan().await;
    let id = f.model("Box").await;
    let first = f.thumb(ThumbTarget::Model(id)).await.unwrap();
    fs::remove_dir_all(f.engine.thumbnails().cache_dir()).unwrap();
    let again = f.thumb(ThumbTarget::Model(id)).await.unwrap();
    assert_eq!(first, again);
    assert!(again.is_file());
}

#[tokio::test]
async fn missing_file_is_unreadable_and_not_remembered() {
    let mut f = Fx::new().await;
    f.write("Gone/g.stl", &cuboid_stl(3.0, 3.0, 3.0));
    f.scan().await;
    let id = f.model("Gone").await;
    fs::remove_file(f.root().join("Gone/g.stl")).unwrap();
    assert_eq!(
        f.thumb(ThumbTarget::Model(id)).await,
        Err(ThumbFailure::Unreadable)
    );
    assert!(walk(f.engine.thumbnails().cache_dir()).is_empty());
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
