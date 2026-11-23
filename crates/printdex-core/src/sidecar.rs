//! `.printdex.toml` sidecars: a mirror of what the user wrote about a model,
//! kept next to the model so a library can be restored without the database
//! (v1 design §6, §8).
//!
//! * A folder model's sidecar is `<folder>/.printdex.toml`; a single-file
//!   model's is `.<file name>.printdex.toml` next to the file.
//! * Writes are debounced per model and atomic (temp file + rename). A
//!   folder that can't be written (read-only, offline) keeps the model
//!   database-only.
//! * On scan, a sidecar whose mtime differs from the one Printdex recorded is
//!   imported record by record; the newest `updated_at` wins and nothing is
//!   deleted.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use crate::db::{Db, Organize, SidecarTarget};
use crate::error::{CoreError, Result};
use crate::grouping::ModelKind;
use crate::ids::{AttemptId, ModelId, Uuid};
use crate::model_data::{
    Attempt, AttemptResult, ModelData, PartOverride, PrintSettings, Recipe, RecipeNote,
};
use crate::time::system_time_to_ns;

/// File name of a folder model's sidecar; single-file models use
/// `.<file name>` + this.
pub const FOLDER_SIDECAR: &str = ".printdex.toml";
/// Sidecar format version this build reads and writes.
pub const SCHEMA: u32 = 1;
/// Largest sidecar Printdex reads or writes.
pub const MAX_BYTES: u64 = 1024 * 1024;
/// Wait after the last change to a model before its sidecar is written.
pub const DEBOUNCE: Duration = Duration::from_secs(1);

/// The sidecar file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidecarFile {
    pub schema: u32,
    /// The Printdex install that wrote the file.
    #[serde(default)]
    pub writer: String,
    #[serde(default)]
    pub model_id: String,
    /// Catalogue number in the library (№ 0412), kept across reinstalls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Last change to notes, source, tags or collections (unix ms).
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipe: Option<SidecarRecipe>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<SidecarAttempt>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SidecarRecipe {
    #[serde(default)]
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub required: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_attempt: Option<String>,
    #[serde(default)]
    pub settings: PrintSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<RecipeNote>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<PartOverride>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SidecarAttempt {
    pub id: String,
    #[serde(default)]
    pub updated_at: i64,
    pub printed_at: i64,
    pub result: AttemptResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_min: Option<u32>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parts: Vec<String>,
    #[serde(default)]
    pub settings: PrintSettings,
}

impl SidecarFile {
    /// The file for a model's data.
    pub fn from_data(install: Uuid, model: ModelId, org: &Organize, data: &ModelData) -> Self {
        SidecarFile {
            schema: SCHEMA,
            writer: install.hyphenated().to_string(),
            model_id: model.to_string(),
            number: None,
            updated_at: org.updated_at,
            notes: org.notes.clone(),
            source: org.source.clone(),
            tags: org.tags.clone(),
            collections: org.collections.clone(),
            recipe: data.recipe.as_ref().map(|r| SidecarRecipe {
                updated_at: r.updated_at,
                required: r.required.clone(),
                from_attempt: r.from_attempt.map(|a| a.to_string()),
                settings: r.settings.clone(),
                notes: r.notes.clone(),
                parts: r.parts.clone(),
            }),
            // Oldest first, so hand edits append at the end.
            attempts: data
                .attempts
                .iter()
                .rev()
                .map(|a| SidecarAttempt {
                    id: a.id.to_string(),
                    updated_at: a.updated_at,
                    printed_at: a.printed_at,
                    result: a.result,
                    duration_min: a.duration_min,
                    notes: a.notes.clone(),
                    parts: a.parts.clone(),
                    settings: a.settings.clone(),
                })
                .collect(),
        }
    }

    /// Whether the file carries no user data.
    pub fn is_empty(&self) -> bool {
        self.notes.is_empty()
            && self.source.is_none()
            && self.tags.is_empty()
            && self.collections.is_empty()
            && self.recipe.is_none()
            && self.attempts.is_empty()
    }

    /// TOML text, with a header comment.
    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string(self).map_err(|e| CoreError::Io {
            path: PathBuf::from(FOLDER_SIDECAR),
            source: std::io::Error::other(e.to_string()),
        })?;
        Ok(format!(
            "# Printdex data for this model: tags, notes, recipe and print log.\n\
             # Printdex keeps this file up to date; edits made here are read on the next scan.\n\n{body}"
        ))
    }

    /// Parses sidecar text. Files from a newer schema are refused.
    pub fn parse(text: &str) -> std::result::Result<SidecarFile, String> {
        let file: SidecarFile = toml::from_str(text).map_err(|e| e.to_string())?;
        if file.schema == 0 || file.schema > SCHEMA {
            return Err(format!("unsupported sidecar schema {}", file.schema));
        }
        Ok(file)
    }

    /// Equal apart from who wrote it.
    fn same_data(&self, other: &SidecarFile) -> bool {
        let strip = |f: &SidecarFile| SidecarFile {
            writer: String::new(),
            model_id: String::new(),
            ..f.clone()
        };
        strip(self) == strip(other)
    }
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .fold(root.to_path_buf(), |p, s| p.join(s))
}

/// Where the sidecar of a model at `rel_path` lives.
pub fn path_for(root: &Path, rel_path: &str, kind: ModelKind) -> PathBuf {
    match kind {
        ModelKind::Folder => join_rel(root, rel_path).join(FOLDER_SIDECAR),
        ModelKind::SingleFile => {
            let (dir, file) = rel_path.rsplit_once('/').unwrap_or(("", rel_path));
            join_rel(root, dir).join(format!(".{file}{FOLDER_SIDECAR}"))
        }
    }
}

/// The model a sidecar at library-relative `rel` belongs to: its path and
/// kind.
pub fn model_for(rel: &str) -> Option<(String, ModelKind)> {
    let (dir, file) = rel.rsplit_once('/').unwrap_or(("", rel));
    if file == FOLDER_SIDECAR {
        return (!dir.is_empty()).then(|| (dir.to_owned(), ModelKind::Folder));
    }
    let name = file.strip_prefix('.')?.strip_suffix(FOLDER_SIDECAR)?;
    if name.is_empty() {
        return None;
    }
    let path = if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    };
    Some((path, ModelKind::SingleFile))
}

/// The current data of model `id` as a sidecar file.
pub async fn snapshot(db: &Db, id: ModelId, install: Uuid) -> Result<Option<SidecarFile>> {
    let (Some(org), Some(data)) = (db.organize(id).await?, db.model_data(id).await?) else {
        return Ok(None);
    };
    let mut file = SidecarFile::from_data(install, id, &org, &data);
    file.number = db.model_number(id).await?;
    Ok(Some(file))
}

/// Writes `text` to `path` atomically and returns the new file's mtime.
/// Blocking.
fn write_atomic(path: &Path, text: &str) -> std::io::Result<i64> {
    if text.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("sidecar too large"));
    }
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent folder"))?;
    // Never create folders: a missing folder means the model moved or the
    // share is offline.
    if !dir.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "model folder not found",
        ));
    }
    set_aside_if_broken(path)?;
    crate::fsx::durable_write(path, text.as_bytes(), crate::fsx::Durability::UserData)?;
    Ok(std::fs::metadata(path)?
        .modified()
        .map(system_time_to_ns)
        .unwrap_or(0))
}

/// Renames an existing sidecar that cannot be read as one (bad TOML, wrong
/// schema, too large) to `.<name>.broken-<unix_ms>` beside it, so the write
/// that follows doesn't destroy the person's file. Other read errors leave
/// the file alone. Best effort: a failed rename is logged and the write
/// goes on. Blocking.
fn set_aside_if_broken(path: &Path) -> std::io::Result<()> {
    use std::io::Read;
    // Broken: too large, not UTF-8, or not a sidecar. Missing: nothing to
    // do. Any other I/O error is returned, so the write doesn't go over a
    // file that couldn't be looked at.
    let broken = match std::fs::File::open(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(f) => {
            if f.metadata()?.len() > MAX_BYTES {
                true
            } else {
                let mut bytes = Vec::new();
                f.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
                match String::from_utf8(bytes) {
                    Ok(text) => bytes_too_big(&text) || SidecarFile::parse(&text).is_err(),
                    Err(_) => true,
                }
            }
        }
    };
    if !broken {
        return Ok(());
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(std::io::Error::other("sidecar has no folder"));
    };
    // Sidecar names start with a dot already.
    let name = name.to_string_lossy();
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    for n in 0..100 {
        let suffix = if n == 0 {
            format!("{at}")
        } else {
            format!("{at}-{n}")
        };
        let to = dir.join(format!("{name}.broken-{suffix}"));
        match crate::fsx::rename_no_replace(path, &to) {
            Ok(()) => {
                tracing::warn!(from = %path.display(), to = %to.display(), "unreadable sidecar set aside");
                return Ok(());
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::warn!(error = %e, path = %path.display(), "could not set aside unreadable sidecar; not overwriting it");
                return Err(e);
            }
        }
    }
    Err(std::io::Error::other(
        "no free name to set the sidecar aside",
    ))
}

fn bytes_too_big(text: &str) -> bool {
    text.len() as u64 > MAX_BYTES
}

/// Reads a sidecar, refusing files over [`MAX_BYTES`]. Blocking.
fn read_capped(path: &Path) -> std::io::Result<(String, i64)> {
    use std::io::Read;
    let f = std::fs::File::open(path)?;
    let meta = f.metadata()?;
    if meta.len() > MAX_BYTES {
        return Err(std::io::Error::other("sidecar too large"));
    }
    let mut text = String::new();
    f.take(MAX_BYTES + 1).read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("sidecar too large"));
    }
    Ok((text, meta.modified().map(system_time_to_ns).unwrap_or(0)))
}

/// Writes the sidecar of model `id` now, when its library writes sidecars.
/// `Ok(false)` when nothing was written (sidecars off, no data and no
/// earlier sidecar, unknown model).
pub async fn write_now(db: &Db, id: ModelId, install: Uuid) -> Result<bool> {
    let Some(target) = db.sidecar_target(id).await? else {
        return Ok(false);
    };
    if !target.enabled {
        return Ok(false);
    }
    let Some(file) = snapshot(db, id, install).await? else {
        return Ok(false);
    };
    if file.is_empty() && target.mtime_ns.is_none() {
        return Ok(false);
    }
    let text = file.to_toml()?;
    let path = sidecar_path(&target);
    let p = path.clone();
    let mtime = tokio::task::spawn_blocking(move || write_atomic(&p, &text))
        .await
        .map_err(|e| CoreError::Io {
            path: path.clone(),
            source: std::io::Error::other(e.to_string()),
        })?
        .map_err(|source| CoreError::Io { path, source })?;
    db.set_sidecar_mtime(id, Some(mtime)).await?;
    Ok(true)
}

fn sidecar_path(t: &SidecarTarget) -> PathBuf {
    path_for(&t.root, &t.rel_path, t.kind)
}

/// What importing one sidecar did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imported {
    /// Written by another Printdex install.
    pub foreign: bool,
    /// The database has data the file lacks, so the file should be
    /// rewritten.
    pub needs_write: bool,
}

/// Imports `file` into model `id`, record by record (newest wins).
/// `predecessors` are earlier ids of this install (before a restore):
/// files they wrote count as this install's own.
pub async fn import(
    db: &Db,
    id: ModelId,
    file: &SidecarFile,
    install: Uuid,
    predecessors: &[String],
) -> Result<Imported> {
    let current = db.organize(id).await?.ok_or(CoreError::ModelNotFound)?;
    if let Some(n) = file.number.filter(|n| *n > 0) {
        // Only a model without a number takes it, and only when it is free.
        db.claim_number(id, n).await?;
    }
    if file.updated_at > current.updated_at {
        db.import_organize(
            id,
            &Organize {
                notes: file.notes.clone(),
                source: file.source.clone(),
                tags: file.tags.clone(),
                collections: file.collections.clone(),
                updated_at: file.updated_at,
            },
        )
        .await?;
    }
    if let Some(r) = &file.recipe {
        let existing = db.model_data(id).await?.and_then(|d| d.recipe);
        if existing.is_none_or(|e| r.updated_at > e.updated_at) {
            let recipe = Recipe {
                settings: r.settings.clone(),
                required: r.required.clone(),
                notes: r.notes.clone(),
                parts: r.parts.clone(),
                from_attempt: r
                    .from_attempt
                    .as_deref()
                    .and_then(|s| Uuid::parse_str(s).ok())
                    .map(AttemptId),
                updated_at: r.updated_at,
            };
            if let Err(e) = db.import_recipe(id, &recipe).await {
                tracing::warn!(model = %id, error = %e, "skipping invalid recipe in sidecar");
            }
        }
    }
    for a in file.attempts.iter().take(10_000) {
        let Ok(uuid) = Uuid::parse_str(&a.id) else {
            continue;
        };
        let attempt = Attempt {
            id: AttemptId(uuid),
            printed_at: a.printed_at,
            result: a.result,
            settings: a.settings.clone(),
            duration_min: a.duration_min,
            notes: a.notes.clone(),
            parts: a.parts.clone(),
            photos: Vec::new(),
            number: 0,
            updated_at: a.updated_at,
        };
        db.import_attempt(id, &attempt).await?;
    }
    let merged = snapshot(db, id, install).await?.unwrap_or_default();
    Ok(Imported {
        foreign: !file.writer.is_empty()
            && file.writer != install.hyphenated().to_string()
            && !predecessors.contains(&file.writer),
        needs_write: !merged.same_data(file),
    })
}

/// Result of importing a library's sidecars.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub imported: u64,
    pub foreign: bool,
    /// Models whose sidecar should be rewritten with merged data.
    pub rewrite: Vec<ModelId>,
}

/// Imports every sidecar in `sidecars` (library-relative path, mtime) whose
/// mtime differs from the one recorded for its model. Unreadable or broken
/// files are logged and skipped.
pub async fn import_library(
    db: &Db,
    root: &Path,
    lib: crate::ids::LibraryId,
    sidecars: &[(String, i64)],
    install: Uuid,
) -> Result<ImportSummary> {
    let mut summary = ImportSummary::default();
    if sidecars.is_empty() {
        return Ok(summary);
    }
    let predecessors = db.install_predecessors().await?;
    let states: HashMap<(String, ModelKind), _> = db
        .sidecar_states(lib)
        .await?
        .into_iter()
        .map(|s| ((s.rel_path.clone(), s.kind), s))
        .collect();
    for (rel, mtime) in sidecars {
        let Some(key) = model_for(rel) else { continue };
        let Some(state) = states.get(&key) else {
            continue;
        };
        if state.mtime_ns == Some(*mtime) {
            continue;
        }
        let path = join_rel(root, rel);
        let p = path.clone();
        let read = tokio::task::spawn_blocking(move || read_capped(&p))
            .await
            .map_err(|e| std::io::Error::other(e.to_string()));
        let (text, real_mtime) = match read {
            Ok(Ok(v)) => v,
            Ok(Err(e)) | Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read sidecar, skipping");
                continue;
            }
        };
        let file = match SidecarFile::parse(&text) {
            Ok(f) => f,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot parse sidecar, skipping");
                continue;
            }
        };
        let own = file.writer.is_empty()
            || file.writer == install.hyphenated().to_string()
            || predecessors.contains(&file.writer);
        if !own && db.syncs_library_with(lib, &file.writer).await? {
            // That install syncs this library with us: its changes arrive
            // through the sync log, which knows which field is newer.
            db.set_sidecar_mtime(state.id, Some(real_mtime)).await?;
            continue;
        }
        let imported = import(db, state.id, &file, install, &predecessors).await?;
        db.set_sidecar_mtime(state.id, Some(real_mtime)).await?;
        summary.imported += 1;
        summary.foreign |= imported.foreign;
        if imported.needs_write {
            summary.rewrite.push(state.id);
        }
    }
    Ok(summary)
}

enum Cmd {
    Queue(ModelId),
    Flush(oneshot::Sender<()>),
}

/// Debounced background sidecar writes. Cheap to clone.
#[derive(Clone, Debug)]
pub struct SidecarWriter {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl std::fmt::Debug for Cmd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Cmd::Queue(id) => write!(f, "Queue({id})"),
            Cmd::Flush(_) => f.write_str("Flush"),
        }
    }
}

impl SidecarWriter {
    /// Starts the writer task. Must be called inside a tokio runtime.
    pub fn start(db: Db, install: Uuid, delay: Duration) -> SidecarWriter {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(run(db, install, delay, rx));
        SidecarWriter { tx }
    }

    /// Writes model `id`'s sidecar after the debounce delay.
    pub fn queue(&self, id: ModelId) {
        let _ = self.tx.send(Cmd::Queue(id));
    }

    /// Writes everything queued now and waits until it is done.
    pub async fn flush(&self) {
        let (done, wait) = oneshot::channel();
        if self.tx.send(Cmd::Flush(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

async fn run(db: Db, install: Uuid, delay: Duration, mut rx: mpsc::UnboundedReceiver<Cmd>) {
    let mut pending: HashMap<ModelId, Instant> = HashMap::new();
    loop {
        let next = pending.values().min().copied();
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(Cmd::Queue(id)) => {
                    pending.insert(id, Instant::now() + delay);
                }
                Some(Cmd::Flush(done)) => {
                    write_due(&db, install, &mut pending, None).await;
                    let _ = done.send(());
                }
                None => {
                    write_due(&db, install, &mut pending, None).await;
                    return;
                }
            },
            () = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                write_due(&db, install, &mut pending, Some(Instant::now())).await;
            }
        }
    }
}

/// Writes pending models due by `now` (all of them with `None`).
async fn write_due(
    db: &Db,
    install: Uuid,
    pending: &mut HashMap<ModelId, Instant>,
    now: Option<Instant>,
) {
    let due: Vec<ModelId> = pending
        .iter()
        .filter(|(_, at)| now.is_none_or(|n| **at <= n))
        .map(|(id, _)| *id)
        .collect();
    for id in due {
        pending.remove(&id);
        match write_now(db, id, install).await {
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(model = %id, error = %e, "cannot write sidecar; keeping data in the database only");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_data::{Material, Supports};

    #[test]
    fn folder_and_file_model_paths() {
        let root = Path::new("/lib");
        assert_eq!(
            path_for(root, "Bins/Bin 2x3", ModelKind::Folder),
            Path::new("/lib/Bins/Bin 2x3/.printdex.toml")
        );
        assert_eq!(
            path_for(root, "Misc/hook.stl", ModelKind::SingleFile),
            Path::new("/lib/Misc/.hook.stl.printdex.toml")
        );
        assert_eq!(
            path_for(root, "hook.stl", ModelKind::SingleFile),
            Path::new("/lib/.hook.stl.printdex.toml")
        );
        assert_eq!(
            model_for("Bins/Bin 2x3/.printdex.toml"),
            Some(("Bins/Bin 2x3".into(), ModelKind::Folder))
        );
        assert_eq!(
            model_for("Misc/.hook.stl.printdex.toml"),
            Some(("Misc/hook.stl".into(), ModelKind::SingleFile))
        );
        assert_eq!(
            model_for(".hook.stl.printdex.toml"),
            Some(("hook.stl".into(), ModelKind::SingleFile))
        );
        assert_eq!(model_for(".printdex.toml"), None);
        assert_eq!(model_for("a/..printdex.toml"), None);
    }

    fn sample() -> SidecarFile {
        SidecarFile {
            schema: SCHEMA,
            writer: "w".into(),
            model_id: "m".into(),
            number: Some(7),
            updated_at: 5,
            notes: "Remixed.\n\nUse the 3MF.".into(),
            source: Some("https://printables.com/model/1".into()),
            tags: vec!["gridfinity".into(), "functional".into()],
            collections: vec!["Desk organizers".into()],
            recipe: Some(SidecarRecipe {
                updated_at: 7,
                required: ["material".to_owned()].into(),
                from_attempt: None,
                settings: PrintSettings {
                    material: Some(Material::Petg),
                    layer_mm: Some(0.2),
                    supports: Some(Supports::None),
                    bed_temp: Some(85),
                    custom: [("Ironing".to_owned(), "on".to_owned())].into(),
                    ..PrintSettings::default()
                },
                notes: vec![RecipeNote {
                    material: Some(Material::Petg),
                    text: "Warps unless the bed is at 85 °C.".into(),
                }],
                parts: vec![PartOverride {
                    part: "label.stl".into(),
                    settings: PrintSettings {
                        orientation: Some("face down".into()),
                        ..PrintSettings::default()
                    },
                }],
            }),
            attempts: vec![SidecarAttempt {
                id: AttemptId::new().to_string(),
                updated_at: 9,
                printed_at: 8,
                result: AttemptResult::Failed,
                duration_min: Some(161),
                notes: "Corners lifted".into(),
                parts: vec![],
                settings: PrintSettings {
                    material: Some(Material::Petg),
                    nozzle_mm: Some(0.4),
                    ..PrintSettings::default()
                },
            }],
        }
    }

    #[test]
    fn sidecar_round_trips() {
        let f = sample();
        let text = f.to_toml().unwrap();
        assert!(text.contains("layer_mm = 0.2\n"), "{text}");
        assert!(text.contains("nozzle_mm = 0.4\n"), "{text}");
        assert_eq!(SidecarFile::parse(&text).unwrap(), f);
    }

    #[test]
    fn bad_sidecars_are_refused() {
        assert!(SidecarFile::parse("garbage = [").is_err());
        assert!(SidecarFile::parse("schema = 99").is_err());
        assert!(SidecarFile::parse("schema = 1\ntags = 3").is_err());
        assert!(SidecarFile::parse("tags = []").is_err());
        assert!(SidecarFile::parse("schema = 1").unwrap().is_empty());
    }
}
