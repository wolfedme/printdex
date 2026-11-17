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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};
use std::sync::{Arc, Mutex, OnceLock};

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
/// Default size limit of the thumbnail cache.
pub const DEFAULT_CACHE_LIMIT: u64 = 1 << 30;
/// Eviction trims the cache to this share of the limit, so it does not
/// run again after every new thumbnail.
const EVICT_TO_PERCENT: u64 = 90;
/// A cache hit refreshes the file's time (for least-recently-used
/// eviction) at most this often.
const TOUCH_AFTER: Duration = Duration::from_secs(3600);
/// Triangle budget of the "full detail" 3D preview. Beyond this, even full
/// detail is decimated, so a 50M-triangle file can't exhaust GPU memory.
pub const FULL_DETAIL_TRIANGLES: usize = 8_000_000;

/// How much of a mesh the 3D preview loads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Detail {
    /// Decimated to [`printdex_mesh::PREVIEW_TRIANGLES`] for smooth orbiting.
    Preview,
    /// Every triangle, up to [`FULL_DETAIL_TRIANGLES`].
    Full,
}

/// A part's mesh, ready for the 3D viewer.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub mesh: printdex_mesh::Mesh,
    /// Size along X, Y and Z in mm, of the full mesh.
    pub dims: [f32; 3],
    /// Triangles in the file.
    pub triangles: u64,
    /// Whether `mesh` has fewer triangles than the file.
    pub decimated: bool,
}

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

    /// Nothing pending or running: a batch has finished.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.running.is_empty()
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
    gpu: Arc<Gpu>,
    /// Cache size limit in bytes.
    limit: AtomicU64,
    /// Something was written since the last eviction check.
    dirty: AtomicBool,
}

/// The thumbnail GPU, opened on first use. A render that fails on the GPU
/// is redone on the CPU, and the GPU is not used again this session.
#[derive(Debug, Default)]
pub(crate) struct Gpu {
    renderer: OnceLock<Option<printdex_render::GpuRenderer>>,
    broken: AtomicBool,
}

impl Gpu {
    /// The GPU renderer, or `None` to render on the CPU. Blocking on first
    /// call (opens the device); call it on a worker.
    fn get(&self) -> Option<&printdex_render::GpuRenderer> {
        if self.broken.load(Ordering::Relaxed) {
            return None;
        }
        self.renderer
            .get_or_init(|| {
                let gpu = printdex_render::GpuRenderer::new();
                match &gpu {
                    Some(g) => {
                        tracing::info!(adapter = %g.description, "thumbnails render on the GPU")
                    }
                    None => tracing::info!("no suitable GPU; thumbnails render on the CPU"),
                }
                gpu
            })
            .as_ref()
    }

    /// Renders on the GPU when there is one, else (or on any GPU failure)
    /// on the CPU.
    fn render(
        &self,
        mesh: &printdex_mesh::Mesh,
        opts: &printdex_render::RenderOptions,
    ) -> printdex_render::Image {
        if let Some(gpu) = self.get() {
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| gpu.render(mesh, opts)));
            match result {
                Ok(Ok(img)) => return img,
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "GPU thumbnail failed; using the CPU from now on")
                }
                Err(_) => tracing::warn!("GPU thumbnail panicked; using the CPU from now on"),
            }
            self.broken.store(true, Ordering::Relaxed);
        }
        printdex_render::render(mesh, opts)
    }
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
            gpu: Arc::new(Gpu::default()),
            limit: AtomicU64::new(DEFAULT_CACHE_LIMIT),
            dirty: AtomicBool::new(true),
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

    /// Loads part `id`'s mesh for the 3D viewer, decimated for `detail`.
    /// Runs on the worker pool under the same memory budget as
    /// thumbnails; dropping the future drops the result.
    pub async fn load_preview(&self, id: PartId, detail: Detail) -> Result<Preview, ThumbFailure> {
        let src = match self.inner.db.part_source(id).await {
            Ok(Some(src)) => src,
            Ok(None) => return Err(ThumbFailure::NoSource),
            Err(e) => {
                tracing::warn!(%id, error = %e, "preview lookup failed");
                return Err(ThumbFailure::Unreadable);
            }
        };
        let Ok(_permit) = self
            .inner
            .budget
            .acquire_many(weight(src.format, src.size))
            .await
        else {
            return Err(ThumbFailure::Unreadable);
        };
        let target = match detail {
            Detail::Preview => printdex_mesh::PREVIEW_TRIANGLES,
            Detail::Full => FULL_DETAIL_TRIANGLES,
        };
        let (path, format) = (src.path.clone(), src.format);
        run_cpu(move || {
            let mesh = load_mesh(&path, format)?;
            let bounds = mesh.bounds().ok_or(ThumbFailure::Invalid)?;
            let triangles = mesh.triangles();
            let mesh = printdex_mesh::decimate(mesh, target);
            Ok(Preview {
                decimated: mesh.triangles() < triangles,
                mesh,
                dims: bounds.size(),
                triangles: triangles as u64,
            })
        })
        .await
    }

    /// Sets the cache size limit and trims the cache to it.
    pub async fn set_cache_limit(&self, bytes: u64) {
        self.inner.limit.store(bytes, Ordering::Relaxed);
        self.inner.dirty.store(true, Ordering::Relaxed);
        self.inner.evict_if_needed().await;
    }

    pub fn cache_limit(&self) -> u64 {
        self.inner.limit.load(Ordering::Relaxed)
    }

    /// Bytes the thumbnail cache uses on disk.
    pub async fn cache_usage(&self) -> u64 {
        let root = self.inner.root().to_owned();
        tokio::task::spawn_blocking(move || cache_files(&root).iter().map(|f| f.size).sum())
            .await
            .unwrap_or(0)
    }

    /// Deletes every cached thumbnail; they are rendered again when next
    /// shown.
    pub async fn clear_cache(&self) -> std::io::Result<()> {
        let root = self.inner.root().to_owned();
        tokio::task::spawn_blocking(move || match std::fs::remove_dir_all(&root) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        })
        .await
        .map_err(std::io::Error::other)?
    }
}

/// One file in the cache.
#[derive(Debug)]
struct CacheFile {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

/// Every file under `root`, recursively.
fn cache_files(root: &Path) -> Vec<CacheFile> {
    let mut out = Vec::new();
    let mut dirs = vec![root.to_owned()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let Ok(meta) = e.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(e.path());
            } else {
                out.push(CacheFile {
                    path: e.path(),
                    size: meta.len(),
                    used: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                });
            }
        }
    }
    out
}

/// Deletes the least recently used files under `root` until it holds at
/// most `EVICT_TO_PERCENT` of `limit`, if it holds more than `limit`.
/// Returns the bytes freed.
fn evict(root: &Path, limit: u64) -> u64 {
    let mut files = cache_files(root);
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    if total <= limit {
        return 0;
    }
    let goal = limit / 100 * EVICT_TO_PERCENT;
    files.sort_by_key(|f| f.used);
    let mut freed = 0;
    for f in files {
        if total <= goal {
            break;
        }
        if std::fs::remove_file(&f.path).is_ok() {
            total -= f.size;
            freed += f.size;
        }
    }
    freed
}

/// Marks a cached file as just used, unless it was recently.
fn touch(path: &Path) {
    let now = SystemTime::now();
    let stale = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > TOUCH_AFTER);
    if stale && let Ok(f) = std::fs::File::options().append(true).open(path) {
        let _ = f.set_modified(now);
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
        let idle = {
            let mut q = lock(&inner.queue);
            q.done(target);
            q.is_idle()
        };
        if idle {
            inner.evict_if_needed().await;
        }
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

fn load_mesh(path: &Path, format: PrintableFormat) -> Result<printdex_mesh::Mesh, ThumbFailure> {
    use printdex_mesh::MeshError;
    printdex_mesh::load(path, mesh_format(format), printdex_mesh::Limits::default()).map_err(|e| {
        match e {
            MeshError::Io(_) => ThumbFailure::Unreadable,
            MeshError::TooLarge { .. } => ThumbFailure::TooLarge,
            MeshError::Malformed(_) => ThumbFailure::Invalid,
        }
    })
}

fn render_part(path: &Path, format: PrintableFormat, gpu: &Gpu) -> Result<Rendered, ThumbFailure> {
    let mesh = load_mesh(path, format)?;
    let Some(bounds) = mesh.bounds() else {
        return Err(ThumbFailure::Invalid);
    };
    let img = gpu.render(
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
    /// The whole thumbnail cache, every look version.
    fn root(&self) -> &Path {
        self.dir.parent().unwrap_or(&self.dir)
    }

    /// Trims the cache to its limit when something was written since the
    /// last check.
    async fn evict_if_needed(&self) {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return;
        }
        let (root, limit) = (self.root().to_owned(), self.limit.load(Ordering::Relaxed));
        let freed = tokio::task::spawn_blocking(move || evict(&root, limit))
            .await
            .unwrap_or(0);
        if freed > 0 {
            tracing::info!(freed, limit, "trimmed the thumbnail cache");
        }
    }

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
                touch(&png);
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
        self.dirty.store(true, Ordering::Relaxed);
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
        let (path, format, gpu) = (src.path.clone(), src.format, self.gpu.clone());
        match run_cpu(move || render_part(&path, format, &gpu)).await {
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
