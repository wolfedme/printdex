//! Thumbnails: a queue fed by what is on screen, a cache keyed by file
//! content, and `ThumbnailReady` events.
//!
//! The app calls [`ThumbnailService::request_visible`] with everything it
//! shows that has no thumbnail yet. That call never blocks: it replaces the
//! queue (items that left the screen are dropped, jobs already running
//! finish) and wakes the workers. Workers check the cache first, then parse
//! and render on the rayon pool, write the PNG atomically, and announce it.
//!
//! Sources, in order (v1 design §7): a 3MF part's embedded thumbnail, then
//! (for a model's cover) an image in the model folder, then a render of the
//! part. Cache files are named after the source's identity (part
//! fingerprint, or image path, size and mtime), so a moved file reuses its
//! thumbnail and a changed file gets a new one. The cache folder is safe to
//! delete.

use std::collections::{HashSet, VecDeque};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tokio::sync::{Notify, Semaphore, broadcast};
use xxhash_rust::xxh3::xxh3_64;

use crate::cpu::run_cpu;
use crate::db::{Db, ImageSource, ModelSources, PartSource};
use crate::events::EngineEvent;
use crate::ids::{ModelId, PartId};
use crate::walk::PrintableFormat;

/// Thumbnail size in pixels. The grid shows them at up to about 1.3× this
/// size on a 2× display, the model view's tiles much smaller.
pub const THUMB_WIDTH: u32 = 384;
pub const THUMB_HEIGHT: u32 = 288;
/// Bumped when the look of rendered thumbnails changes; old files are then
/// simply ignored.
const CACHE_VERSION: &str = "v1";
/// Mesh memory allowed in flight across all jobs, in MiB.
pub const BUDGET_MIB: u32 = 768;
/// Largest folder image decoded for a cover.
const MAX_IMAGE_BYTES: u64 = 64 << 20;

/// Something that has a thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThumbTarget {
    /// A model's cover.
    Model(ModelId),
    /// One part (printable file).
    Part(PartId),
}

/// Where a thumbnail came from. Renders have a transparent background and
/// stand on the bed line (the app draws a contact shadow under them);
/// embedded 3MF pictures and folder images are opaque pictures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThumbKind {
    Render,
    Embedded,
    Image,
}

impl ThumbKind {
    /// From a cache file name: keys start with `p`, `e` or `i`.
    fn of(path: &Path) -> ThumbKind {
        match path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.chars().next())
        {
            Some('e') => ThumbKind::Embedded,
            Some('i') => ThumbKind::Image,
            _ => ThumbKind::Render,
        }
    }
}

/// Why there is no thumbnail. Codes, not sentences, so the UI can
/// translate them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbFailure {
    /// The file could not be read right now (offline share, permissions).
    /// Not remembered: the next request tries again.
    Unreadable,
    /// The file is not a valid mesh or image.
    Invalid,
    /// More triangles than Printdex previews.
    TooLarge,
    /// The model has nothing to preview.
    NoSource,
}

impl ThumbFailure {
    fn code(&self) -> &'static str {
        match self {
            ThumbFailure::Unreadable => "unreadable",
            ThumbFailure::Invalid => "invalid",
            ThumbFailure::TooLarge => "too-large",
            ThumbFailure::NoSource => "no-source",
        }
    }

    fn from_code(s: &str) -> ThumbFailure {
        match s.trim() {
            "too-large" => ThumbFailure::TooLarge,
            "no-source" => ThumbFailure::NoSource,
            _ => ThumbFailure::Invalid,
        }
    }
}

/// Pending and running jobs. Pure bookkeeping, so it is unit tested
/// without threads.
#[derive(Debug, Default)]
pub(crate) struct Queue {
    pending: VecDeque<ThumbTarget>,
    running: HashSet<ThumbTarget>,
}

impl Queue {
    /// Replaces the pending jobs with `targets`, in order, without
    /// duplicates or jobs already running.
    pub fn set_visible(&mut self, targets: impl IntoIterator<Item = ThumbTarget>) {
        let mut seen = HashSet::new();
        self.pending = targets
            .into_iter()
            .filter(|t| !self.running.contains(t) && seen.insert(*t))
            .collect();
    }

    /// Takes the next job and marks it running.
    pub fn next(&mut self) -> Option<ThumbTarget> {
        let t = self.pending.pop_front()?;
        self.running.insert(t);
        Some(t)
    }

    pub fn done(&mut self, t: ThumbTarget) {
        self.running.remove(&t);
    }

    #[cfg(test)]
    fn pending(&self) -> Vec<ThumbTarget> {
        self.pending.iter().copied().collect()
    }
}

/// Memory weight of loading a file, in MiB, capped at the whole budget.
pub(crate) fn weight(format: PrintableFormat, size: u64) -> u32 {
    // Rough bytes in memory per byte on disk: binary STL holds 36 of every
    // 50 bytes plus projected positions; 3MF is compressed XML.
    let factor = match format {
        PrintableFormat::Stl => 1.5,
        PrintableFormat::Obj => 1.0,
        PrintableFormat::ThreeMf => 6.0,
    };
    let mib = (size as f64 * factor / f64::from(1u32 << 20)).ceil();
    (mib as u32).clamp(1, BUDGET_MIB)
}

/// Handle to the thumbnail workers. Cheap to clone.
#[derive(Clone, Debug)]
pub struct ThumbnailService {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    db: Db,
    events: broadcast::Sender<EngineEvent>,
    dir: PathBuf,
    queue: Mutex<Queue>,
    wake: Notify,
    budget: Semaphore,
}

/// What a job produced.
enum Outcome {
    Ready(PathBuf),
    Failed(ThumbFailure),
    /// The model or part no longer exists.
    Gone,
}

impl ThumbnailService {
    /// Starts the workers. Must be called inside a tokio runtime.
    pub(crate) fn start(
        db: Db,
        events: broadcast::Sender<EngineEvent>,
        cache_dir: &Path,
    ) -> ThumbnailService {
        let inner = Arc::new(Inner {
            db,
            events,
            dir: cache_dir.join("thumbs").join(CACHE_VERSION),
            queue: Mutex::new(Queue::default()),
            wake: Notify::new(),
            budget: Semaphore::new(BUDGET_MIB as usize),
        });
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get());
        for _ in 0..(cores / 2).clamp(1, 4) {
            tokio::spawn(worker(inner.clone()));
        }
        ThumbnailService { inner }
    }

    /// Makes `targets` (on screen, most important first) the work to do.
    /// Returns at once; results arrive as events.
    pub fn request_visible(&self, targets: impl IntoIterator<Item = ThumbTarget>) {
        lock(&self.inner.queue).set_visible(targets);
        self.inner.wake.notify_waiters();
    }

    /// Folder holding the cached thumbnails.
    pub fn cache_dir(&self) -> &Path {
        &self.inner.dir
    }
}

fn lock(q: &Mutex<Queue>) -> std::sync::MutexGuard<'_, Queue> {
    q.lock().unwrap_or_else(|p| p.into_inner())
}

async fn worker(inner: Arc<Inner>) {
    loop {
        let target = loop {
            let notified = inner.wake.notified();
            if let Some(t) = lock(&inner.queue).next() {
                break t;
            }
            notified.await;
        };
        let outcome = match target {
            ThumbTarget::Part(id) => inner.part(id).await,
            ThumbTarget::Model(id) => inner.model(id).await,
        };
        lock(&inner.queue).done(target);
        let event = match outcome {
            Outcome::Ready(path) => EngineEvent::ThumbnailReady {
                target,
                kind: ThumbKind::of(&path),
                path,
            },
            Outcome::Failed(reason) => EngineEvent::ThumbnailFailed { target, reason },
            Outcome::Gone => continue,
        };
        let _ = inner.events.send(event);
    }
}

/// Cache key of a part's file content.
fn part_key(p: &PartSource) -> String {
    let content = p
        .fp_hash
        .unwrap_or_else(|| xxh3_64(&p.mtime_ns.to_le_bytes()));
    format!("p{:016x}{:016x}", p.size, content)
}

fn image_key(i: &ImageSource) -> String {
    let mut buf = i.rel_path.as_bytes().to_vec();
    buf.extend(i.size.to_le_bytes());
    buf.extend(i.mtime_ns.to_le_bytes());
    format!("i{:016x}", xxh3_64(&buf))
}

/// The image that best serves as a cover: names that say so first, then
/// the first by path.
fn best_image(images: &[ImageSource]) -> Option<&ImageSource> {
    const HINTS: &[&str] = &["cover", "preview", "thumb", "render"];
    images
        .iter()
        .find(|i| {
            let name = i.rel_path.rsplit('/').next().unwrap_or("").to_lowercase();
            HINTS.iter().any(|h| name.contains(h))
        })
        .or_else(|| images.first())
}

fn mesh_format(f: PrintableFormat) -> printdex_mesh::Format {
    match f {
        PrintableFormat::Stl => printdex_mesh::Format::Stl,
        PrintableFormat::ThreeMf => printdex_mesh::Format::ThreeMf,
        PrintableFormat::Obj => printdex_mesh::Format::Obj,
    }
}

/// Scales any image down to fit the thumbnail box and encodes it as PNG.
fn image_to_png(bytes: &[u8]) -> Result<Vec<u8>, ThumbFailure> {
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ThumbFailure::Invalid)?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(512 << 20);
    reader.limits(limits);
    let img = reader.decode().map_err(|_| ThumbFailure::Invalid)?;
    let small = if img.width() > THUMB_WIDTH || img.height() > THUMB_HEIGHT {
        img.thumbnail(THUMB_WIDTH, THUMB_HEIGHT)
    } else {
        img
    };
    encode(small.to_rgba8())
}

fn encode(img: image::RgbaImage) -> Result<Vec<u8>, ThumbFailure> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .map_err(|_| ThumbFailure::Invalid)?;
    Ok(out.into_inner())
}

/// What rendering a part produced.
struct Rendered {
    png: Vec<u8>,
    dims: [f32; 3],
    triangles: u64,
}

fn render_part(path: &Path, format: PrintableFormat) -> Result<Rendered, ThumbFailure> {
    use printdex_mesh::MeshError;
    let mesh = printdex_mesh::load(path, mesh_format(format), printdex_mesh::Limits::default())
        .map_err(|e| match e {
            MeshError::Io(_) => ThumbFailure::Unreadable,
            MeshError::TooLarge { .. } => ThumbFailure::TooLarge,
            MeshError::Malformed(_) => ThumbFailure::Invalid,
        })?;
    let Some(bounds) = mesh.bounds() else {
        return Err(ThumbFailure::Invalid);
    };
    let img = printdex_render::render(
        &mesh,
        &printdex_render::RenderOptions {
            width: THUMB_WIDTH,
            height: THUMB_HEIGHT,
            ..printdex_render::RenderOptions::default()
        },
    );
    let triangles = mesh.triangles() as u64;
    drop(mesh);
    let rgba =
        image::RgbaImage::from_raw(img.width, img.height, img.rgba).ok_or(ThumbFailure::Invalid)?;
    Ok(Rendered {
        png: encode(rgba)?,
        dims: bounds.size(),
        triangles,
    })
}

impl Inner {
    fn path_for(&self, key: &str, ext: &str) -> PathBuf {
        self.dir.join(&key[1..3]).join(format!("{key}.{ext}"))
    }

    /// The cached result for `key`: a thumbnail, a remembered failure, a
    /// remembered "no embedded thumbnail", or nothing yet.
    async fn cached(&self, key: &str) -> Option<Outcome> {
        let png = self.path_for(key, "png");
        let fail = self.path_for(key, "fail");
        tokio::task::spawn_blocking(move || {
            if png.is_file() {
                Some(Outcome::Ready(png))
            } else {
                std::fs::read_to_string(&fail)
                    .ok()
                    .map(|code| Outcome::Failed(ThumbFailure::from_code(&code)))
            }
        })
        .await
        .ok()
        .flatten()
    }

    /// Writes `bytes` under `key` with extension `ext`, atomically.
    async fn store(&self, key: &str, ext: &str, bytes: Vec<u8>) -> Option<PathBuf> {
        let path = self.path_for(key, ext);
        let tmp = path.with_extension(format!("{ext}.tmp{:08x}", fastrand::u32(..)));
        tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(path.parent()?).ok()?;
            std::fs::write(&tmp, bytes).ok()?;
            if std::fs::rename(&tmp, &path).is_err() {
                let _ = std::fs::remove_file(&tmp);
                return None;
            }
            Some(path)
        })
        .await
        .ok()
        .flatten()
    }

    /// Remembers a failure that will not go away until the file changes.
    async fn remember(&self, key: &str, reason: &ThumbFailure) {
        if *reason != ThumbFailure::Unreadable {
            self.store(key, "fail", reason.code().as_bytes().to_vec())
                .await;
        }
    }

    async fn part(&self, id: PartId) -> Outcome {
        match self.db.part_source(id).await {
            Ok(Some(src)) => self.part_from(&src).await,
            Ok(None) => Outcome::Gone,
            Err(e) => {
                tracing::warn!(%id, error = %e, "thumbnail lookup failed");
                Outcome::Failed(ThumbFailure::Unreadable)
            }
        }
    }

    /// A part's thumbnail: its embedded one for 3MF, else a render.
    async fn part_from(&self, src: &PartSource) -> Outcome {
        if src.format == PrintableFormat::ThreeMf
            && let Some(path) = self.embedded(src).await
        {
            return Outcome::Ready(path);
        }
        self.rendered(src).await
    }

    /// The embedded 3MF thumbnail of `src`, extracting it on first use.
    async fn embedded(&self, src: &PartSource) -> Option<PathBuf> {
        let key = format!("e{}", &part_key(src)[1..]);
        match self.cached(&key).await {
            Some(Outcome::Ready(p)) => return Some(p),
            Some(_) => return None,
            None => {}
        }
        let path = src.path.clone();
        let png = run_cpu(move || {
            let bytes = printdex_mesh::threemf_thumbnail(&path, printdex_mesh::Limits::default())
                .map_err(|_| ThumbFailure::Invalid)?
                .ok_or(ThumbFailure::NoSource)?;
            image_to_png(&bytes)
        })
        .await;
        match png {
            Ok(png) => self.store(&key, "png", png).await,
            Err(reason) => {
                self.remember(&key, &reason).await;
                None
            }
        }
    }

    async fn rendered(&self, src: &PartSource) -> Outcome {
        let key = part_key(src);
        if let Some(hit) = self.cached(&key).await {
            return hit;
        }
        let Ok(_permit) = self.budget.acquire_many(weight(src.format, src.size)).await else {
            return Outcome::Failed(ThumbFailure::Unreadable);
        };
        let (path, format) = (src.path.clone(), src.format);
        match run_cpu(move || render_part(&path, format)).await {
            Ok(r) => {
                if let Err(e) = self
                    .db
                    .set_measurements(src.id, src.fp_hash, r.dims, r.triangles)
                    .await
                {
                    tracing::warn!(error = %e, "storing part measurements failed");
                }
                match self.store(&key, "png", r.png).await {
                    Some(p) => Outcome::Ready(p),
                    None => Outcome::Failed(ThumbFailure::Unreadable),
                }
            }
            Err(reason) => {
                self.remember(&key, &reason).await;
                Outcome::Failed(reason)
            }
        }
    }

    async fn image(&self, img: &ImageSource) -> Outcome {
        let key = image_key(img);
        if let Some(hit) = self.cached(&key).await {
            return hit;
        }
        if img.size > MAX_IMAGE_BYTES {
            return Outcome::Failed(ThumbFailure::TooLarge);
        }
        let path = img.path.clone();
        let png = run_cpu(move || {
            let bytes = std::fs::read(&path).map_err(|_| ThumbFailure::Unreadable)?;
            image_to_png(&bytes)
        })
        .await;
        match png {
            Ok(png) => match self.store(&key, "png", png).await {
                Some(p) => Outcome::Ready(p),
                None => Outcome::Failed(ThumbFailure::Unreadable),
            },
            Err(reason) => {
                self.remember(&key, &reason).await;
                Outcome::Failed(reason)
            }
        }
    }

    async fn model(&self, id: ModelId) -> Outcome {
        let sources = match self.db.model_sources(id).await {
            Ok(Some(s)) => s,
            Ok(None) => return Outcome::Gone,
            Err(e) => {
                tracing::warn!(%id, error = %e, "thumbnail lookup failed");
                return Outcome::Failed(ThumbFailure::Unreadable);
            }
        };
        self.cover(&sources).await
    }

    /// A model's cover: embedded 3MF thumbnail, folder image, render.
    async fn cover(&self, s: &ModelSources) -> Outcome {
        for p in s
            .parts
            .iter()
            .filter(|p| p.format == PrintableFormat::ThreeMf)
        {
            if let Some(path) = self.embedded(p).await {
                return Outcome::Ready(path);
            }
        }
        if let Some(img) = best_image(&s.images)
            && let Outcome::Ready(p) = self.image(img).await
        {
            return Outcome::Ready(p);
        }
        match s.parts.first() {
            Some(p) => self.rendered(p).await,
            None => Outcome::Failed(ThumbFailure::NoSource),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(n: u8) -> ThumbTarget {
        ThumbTarget::Model(ModelId(uuid::Uuid::from_bytes([n; 16])))
    }

    #[test]
    fn visible_set_reorders_queue() {
        let mut q = Queue::default();
        q.set_visible([m(1), m(2), m(3)]);
        assert_eq!(q.next(), Some(m(1)));
        // Scrolled: 3 is now first, 2 left the screen, 1 is still running.
        q.set_visible([m(3), m(1), m(4), m(3)]);
        assert_eq!(q.pending(), vec![m(3), m(4)]);
        q.done(m(1));
        q.set_visible([m(1)]);
        assert_eq!(q.pending(), vec![m(1)]);
    }

    #[test]
    fn budget_weights() {
        assert_eq!(weight(PrintableFormat::Stl, 10), 1);
        assert_eq!(weight(PrintableFormat::Stl, 300 << 20), 450);
        assert_eq!(weight(PrintableFormat::ThreeMf, 1 << 30), BUDGET_MIB);
    }

    #[tokio::test]
    async fn budget_limits_concurrent_bytes() {
        let budget = Semaphore::new(BUDGET_MIB as usize);
        let big = weight(PrintableFormat::Stl, 300 << 20);
        let a = budget.acquire_many(big).await.unwrap();
        assert!(
            budget.try_acquire_many(big).is_err(),
            "two 300 MB STLs at once"
        );
        assert!(budget.try_acquire_many(1).is_ok(), "small files still run");
        drop(a);
        assert!(budget.try_acquire_many(big).is_ok());
    }

    fn img(rel: &str) -> ImageSource {
        ImageSource {
            path: PathBuf::from(rel),
            rel_path: rel.into(),
            size: 1,
            mtime_ns: 1,
        }
    }

    #[test]
    fn cover_image_prefers_names_that_say_so() {
        let imgs = [img("a/build.jpg"), img("a/Cover.PNG"), img("a/z.png")];
        assert_eq!(best_image(&imgs).unwrap().rel_path, "a/Cover.PNG");
        assert_eq!(best_image(&imgs[..1]).unwrap().rel_path, "a/build.jpg");
        assert!(best_image(&[]).is_none());
    }

    #[test]
    fn part_key_follows_content_not_path() {
        let a = PartSource {
            id: PartId::new(),
            path: "/x/a.stl".into(),
            format: PrintableFormat::Stl,
            size: 10,
            mtime_ns: 5,
            fp_hash: Some(99),
        };
        let mut b = a.clone();
        b.path = "/y/moved.stl".into();
        b.mtime_ns = 6;
        assert_eq!(part_key(&a), part_key(&b));
        b.fp_hash = Some(100);
        assert_ne!(part_key(&a), part_key(&b));
    }
}
