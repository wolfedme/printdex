//! File actions: rename, move and trash whole models, on the user's
//! explicit request, with a journal so the last action can be undone and a
//! crash halfway can be reconciled on the next start.
//!
//! Model file contents are never changed: models are only renamed, moved
//! (renamed, or copied, checked and the original trashed when the target
//! is on another drive) or moved to the system trash.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use crate::db::file_ops::OpRow;
pub use crate::db::file_ops::{How, OpKind, OpState, Step};
use crate::engine::{Engine, ScanGuard};
use crate::error::{CoreError, Result};
use crate::events::EngineEvent;
use crate::fsx::{self, faults, faults::Op};
use crate::grouping::ModelKind;
use crate::ids::{LibraryId, ModelId};
use crate::sidecar;
use crate::walk::root_readable;

/// A finished (or undone) file action, for the undo toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpSummary {
    /// Journal id; 0 when nothing had to be done.
    pub id: i64,
    pub kind: OpKind,
    pub state: OpState,
    /// The models' names after the action.
    pub names: Vec<String>,
    /// Whether [`Engine::undo`] can undo it on this system (a trash can
    /// only be restored programmatically on Linux and Windows).
    pub undoable: bool,
}

impl OpSummary {
    fn of(row: &OpRow) -> OpSummary {
        OpSummary {
            id: row.id,
            kind: row.kind,
            state: row.state,
            names: row.steps.iter().map(|s| s.new_name.clone()).collect(),
            undoable: row.state == OpState::Done && (row.kind != OpKind::Trash || CAN_RESTORE),
        }
    }

    fn nothing(kind: OpKind) -> OpSummary {
        OpSummary {
            id: 0,
            kind,
            state: OpState::Done,
            names: Vec::new(),
            undoable: false,
        }
    }
}

/// Whether trashed files can be put back by Printdex on this system.
pub const CAN_RESTORE: bool = cfg!(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
));

/// Characters no file name may contain on any of the three systems.
const FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Names Windows reserves, whatever the extension.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Whether `name` is a file or folder name that works on Linux, macOS and
/// Windows (so a library stays usable from any of them).
pub fn valid_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default();
    !name.is_empty()
        && name.len() <= 255
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name.ends_with('.')
        && !name.ends_with(' ')
        && name.trim() == name
        && !name
            .chars()
            .any(|c| c.is_control() || FORBIDDEN.contains(&c))
        && !RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem))
}

fn join_rel(root: &Path, rel: &str) -> PathBuf {
    rel.split('/')
        .filter(|s| !s.is_empty())
        .fold(root.to_path_buf(), |p, s| p.join(s))
}

fn parent_rel(rel: &str) -> &str {
    rel.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn base(rel: &str) -> &str {
    rel.rsplit_once('/').map_or(rel, |(_, name)| name)
}

fn child(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn inside(path: &str, dir: &str) -> bool {
    path == dir || (path.starts_with(dir) && path[dir.len()..].starts_with('/'))
}

/// Model name for a file or folder name: a single file drops its extension.
fn model_name(file_name: &str, kind: ModelKind) -> String {
    match kind {
        ModelKind::Folder => file_name.to_owned(),
        ModelKind::SingleFile => file_name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map_or(file_name, |(stem, _)| stem)
            .to_owned(),
    }
}

fn io_err(path: &Path, source: io::Error) -> CoreError {
    CoreError::Io {
        path: path.to_path_buf(),
        source,
    }
}

impl Engine {
    /// Renames model `id`'s folder (or file, keeping its extension) to
    /// `new_name`, inside the folder it is in.
    pub async fn rename_model(&self, id: ModelId, new_name: &str) -> Result<OpSummary> {
        let new_name = new_name.trim();
        let d = self.present_model(id).await?;
        let file_name = match d.kind {
            ModelKind::Folder => new_name.to_owned(),
            ModelKind::SingleFile => {
                let ext = base(&d.rel_path).rsplit_once('.').map(|(_, e)| e);
                match ext {
                    Some(ext)
                        if !new_name
                            .to_ascii_lowercase()
                            .ends_with(&format!(".{}", ext.to_ascii_lowercase())) =>
                    {
                        format!("{new_name}.{ext}")
                    }
                    _ => new_name.to_owned(),
                }
            }
        };
        if !valid_name(new_name) || !valid_name(&file_name) {
            return Err(CoreError::NameInvalid);
        }
        let to_rel = child(parent_rel(&d.rel_path), &file_name);
        if to_rel == d.rel_path {
            return Ok(OpSummary::nothing(OpKind::Rename));
        }
        let step = Step {
            model: id,
            kind: d.kind,
            name: d.name.clone(),
            new_name: model_name(&file_name, d.kind),
            from_lib: d.library_id,
            from_rel: d.rel_path.clone(),
            to_lib: d.library_id,
            to_rel,
            how: How::Rename,
        };
        self.execute(OpKind::Rename, vec![step], false).await
    }

    /// Moves models into `folder` (relative to library `to_lib`'s root,
    /// `""` for the root), in the same library or another one.
    pub async fn move_models(
        &self,
        ids: &[ModelId],
        to_lib: LibraryId,
        folder: &str,
    ) -> Result<OpSummary> {
        let folder = folder.trim_matches('/');
        if !folder.is_empty() && !folder.split('/').all(valid_name) {
            return Err(CoreError::NameInvalid);
        }
        self.db.library(to_lib).await?;
        let mut steps = Vec::new();
        for &id in ids {
            let d = self.present_model(id).await?;
            if d.library_id == to_lib && inside(folder, &d.rel_path) {
                // Into itself.
                return Err(CoreError::NameInvalid);
            }
            let to_rel = child(folder, base(&d.rel_path));
            if d.library_id == to_lib && to_rel == d.rel_path {
                continue;
            }
            // Two targets differing only in case would be one file on a
            // case-insensitive drive; refused on every system alike.
            let lower = to_rel.to_lowercase();
            if steps
                .iter()
                .any(|s: &Step| s.to_rel.to_lowercase() == lower)
            {
                return Err(CoreError::NameTaken(base(&to_rel).to_owned()));
            }
            steps.push(Step {
                model: id,
                kind: d.kind,
                name: d.name.clone(),
                new_name: d.name.clone(),
                from_lib: d.library_id,
                from_rel: d.rel_path.clone(),
                to_lib,
                to_rel,
                how: How::Rename,
            });
        }
        self.execute(OpKind::Move, steps, false).await
    }

    /// Moves models to the system trash. Models the user wrote nothing
    /// about are forgotten; the others stay under Missing.
    pub async fn trash_models(&self, ids: &[ModelId]) -> Result<OpSummary> {
        let mut steps = Vec::new();
        for &id in ids {
            let d = self.present_model(id).await?;
            steps.push(Step {
                model: id,
                kind: d.kind,
                name: d.name.clone(),
                new_name: d.name.clone(),
                from_lib: d.library_id,
                from_rel: d.rel_path.clone(),
                to_lib: d.library_id,
                to_rel: d.rel_path.clone(),
                how: How::Trash,
            });
        }
        self.execute(OpKind::Trash, steps, false).await
    }

    /// The newest file action, if it can still be undone.
    pub async fn last_op(&self) -> Result<Option<OpSummary>> {
        Ok(self
            .db
            .last_op()
            .await?
            .map(|r| OpSummary::of(&r))
            .filter(|s| s.undoable))
    }

    /// Undoes file action `op`, which must be the newest one.
    pub async fn undo(&self, op: i64) -> Result<OpSummary> {
        let Some(row) = self.db.last_op().await?.filter(|r| r.id == op) else {
            return Err(CoreError::UndoUnavailable);
        };
        if !OpSummary::of(&row).undoable {
            return Err(CoreError::UndoUnavailable);
        }
        match row.kind {
            OpKind::Rename | OpKind::Move => {
                let back: Vec<Step> = row
                    .steps
                    .iter()
                    .rev()
                    .map(|s| Step {
                        model: s.model,
                        kind: s.kind,
                        name: s.new_name.clone(),
                        new_name: s.name.clone(),
                        from_lib: s.to_lib,
                        from_rel: s.to_rel.clone(),
                        to_lib: s.from_lib,
                        to_rel: s.from_rel.clone(),
                        how: How::Rename,
                    })
                    .collect();
                self.execute(row.kind, back, true).await?;
            }
            OpKind::Trash => self.restore_trashed(&row).await?,
        }
        self.db.set_op_state(row.id, OpState::Undone).await?;
        let summary = OpSummary {
            state: OpState::Undone,
            undoable: false,
            names: row.steps.iter().map(|s| s.name.clone()).collect(),
            ..OpSummary::of(&row)
        };
        let _ = self.events.send(EngineEvent::FileOpDone(summary.clone()));
        Ok(summary)
    }

    /// Finishes or rolls back file actions a crash interrupted: whatever
    /// the disk shows happened is written to the database. Returns the
    /// names of the models whose action was finished. Called on open.
    pub async fn reconcile_ops(&self) -> Result<Vec<String>> {
        let mut finished = Vec::new();
        for op in self.db.planned_ops().await? {
            let roots = self.roots(&op.steps).await?;
            let mut applied = 0;
            let mut rescans: BTreeMap<LibraryId, Vec<String>> = BTreeMap::new();
            for s in &op.steps {
                let from = join_rel(&roots[&s.from_lib], &s.from_rel);
                let to = join_rel(&roots[&s.to_lib], &s.to_rel);
                match s.how {
                    How::Trash => {
                        if !from.exists() && roots[&s.from_lib].exists() {
                            applied += 1;
                            finished.push(s.name.clone());
                            rescans
                                .entry(s.from_lib)
                                .or_default()
                                .push(s.from_rel.clone());
                        }
                    }
                    How::Rename | How::Copy => {
                        let sides = (s.kind == ModelKind::SingleFile).then(|| {
                            (
                                sidecar::path_for(&roots[&s.from_lib], &s.from_rel, s.kind),
                                sidecar::path_for(&roots[&s.to_lib], &s.to_rel, s.kind),
                            )
                        });
                        let how = s.how;
                        let at_target = tokio::task::spawn_blocking(move || {
                            settle_step(&from, &to, how, sides)
                        })
                        .await
                        .unwrap_or(false);
                        if at_target {
                            self.db
                                .relocate_model(
                                    s.model,
                                    &s.from_rel,
                                    s.to_lib,
                                    &s.to_rel,
                                    &s.new_name,
                                )
                                .await?;
                            applied += 1;
                            finished.push(s.new_name.clone());
                        }
                    }
                }
            }
            let state = if applied > 0 {
                OpState::Done
            } else {
                OpState::Failed
            };
            self.db.set_op_state(op.id, state).await?;
            for (lib, paths) in rescans {
                let _ = self
                    .rescan_paths(lib, paths, tokio_util::sync::CancellationToken::new())
                    .await;
            }
        }
        if !finished.is_empty() {
            tracing::info!(?finished, "finished file actions interrupted by a crash");
        }
        Ok(finished)
    }

    async fn present_model(&self, id: ModelId) -> Result<crate::db::ModelDetail> {
        self.db
            .model_detail(id)
            .await?
            .filter(|d| d.missing_since.is_none())
            .ok_or(CoreError::ModelNotFound)
    }

    async fn roots(&self, steps: &[Step]) -> Result<HashMap<LibraryId, PathBuf>> {
        let mut roots = HashMap::new();
        for s in steps {
            for lib in [s.from_lib, s.to_lib] {
                if let std::collections::hash_map::Entry::Vacant(e) = roots.entry(lib) {
                    e.insert(self.db.library(lib).await?.path);
                }
            }
        }
        Ok(roots)
    }

    /// Checks, journals and performs `steps`, then moves the rows. With
    /// `reverse`, the new journal entry is an undo (not undoable itself).
    async fn execute(&self, kind: OpKind, steps: Vec<Step>, reverse: bool) -> Result<OpSummary> {
        if steps.is_empty() {
            return Ok(OpSummary::nothing(kind));
        }
        let roots = self.roots(&steps).await?;
        let mut libs: Vec<LibraryId> = roots.keys().copied().collect();
        libs.sort();
        let mut guards: Vec<ScanGuard<'_>> = Vec::new();
        for lib in &libs {
            guards.push(self.claim(*lib)?);
        }

        // Check everything before touching anything.
        for s in &steps {
            let from_root = &roots[&s.from_lib];
            if !root_readable(&*self.fs, from_root) {
                return Err(CoreError::LibraryOffline(from_root.clone()));
            }
            if self.fs.metadata(&join_rel(from_root, &s.from_rel)).is_err() {
                return Err(CoreError::ModelNotFound);
            }
            if s.how == How::Trash {
                continue;
            }
            if !root_readable(&*self.fs, &roots[&s.to_lib]) {
                return Err(CoreError::TargetOffline);
            }
            let case_only =
                s.from_lib == s.to_lib && s.from_rel.to_lowercase() == s.to_rel.to_lowercase();
            let taken = (!case_only
                && self
                    .fs
                    .metadata(&join_rel(&roots[&s.to_lib], &s.to_rel))
                    .is_ok())
                || self.db.path_taken(s.to_lib, &s.to_rel, s.model).await?;
            if taken {
                return Err(CoreError::NameTaken(base(&s.to_rel).to_owned()));
            }
            // A single file's sidecar moves with it and must not land on
            // another one.
            if s.kind == ModelKind::SingleFile && !case_only {
                let side_from = sidecar::path_for(from_root, &s.from_rel, s.kind);
                let side_to = sidecar::path_for(&roots[&s.to_lib], &s.to_rel, s.kind);
                if self.fs.metadata(&side_from).is_ok() && self.fs.metadata(&side_to).is_ok() {
                    return Err(CoreError::NameTaken(file_name(&side_to)));
                }
            }
        }

        let op = self.db.insert_op(kind, &steps).await?;
        let work = steps.clone();
        let work_roots = roots.clone();
        let (handle, db) = (tokio::runtime::Handle::current(), self.db.clone());
        let outcome = tokio::task::spawn_blocking(move || {
            let mut done: Vec<How> = Vec::new();
            for (i, s) in work.iter().enumerate() {
                // Journaled before a copy starts, so that after a crash
                // `reconcile_ops` knows a copy at the target is ours.
                let journal_copy = || {
                    handle
                        .block_on(db.set_step_how(op, i, How::Copy))
                        .map_err(|e| io::Error::other(e.to_string()))
                };
                match do_step(
                    &work_roots[&s.from_lib],
                    &work_roots[&s.to_lib],
                    s,
                    &journal_copy,
                ) {
                    Ok(how) => done.push(how),
                    Err(f) => {
                        let mut stuck: Vec<(usize, How)> =
                            f.stuck.map(|h| (i, h)).into_iter().collect();
                        // Put back what was already done, newest first.
                        for (j, (s, how)) in work.iter().zip(&done).enumerate().rev() {
                            if !undo_step(&work_roots[&s.from_lib], &work_roots[&s.to_lib], s, *how)
                            {
                                stuck.push((j, *how));
                            }
                        }
                        return Err((i, f.path, f.err, stuck));
                    }
                }
            }
            Ok(done)
        })
        .await
        .map_err(|e| io_err(&roots[&libs[0]], io::Error::other(e.to_string())))?;
        let done = match outcome {
            Ok(done) => done,
            Err((i, path, e, stuck)) => {
                // Steps that could not be put back stay where they went; the
                // database follows the disk.
                for (j, how) in &stuck {
                    let s = &steps[*j];
                    if *how != s.how {
                        self.db.set_step_how(op, *j, *how).await?;
                    }
                    self.db
                        .relocate_model(s.model, &s.from_rel, s.to_lib, &s.to_rel, &s.new_name)
                        .await?;
                }
                self.db.set_op_state(op, OpState::Failed).await?;
                if stuck.is_empty() {
                    tracing::warn!(path = %path.display(), error = %e, "file action failed; undone");
                } else {
                    tracing::error!(
                        path = %path.display(),
                        error = %e,
                        stuck = stuck.len(),
                        "file action failed; some models could not be put back and stay at their new place"
                    );
                    let _ = self.events.send(EngineEvent::OrganizeChanged);
                }
                return Err(step_error(&path, e, &steps[i]));
            }
        };
        for (seq, how) in done.iter().enumerate() {
            if *how != steps[seq].how {
                self.db.set_step_how(op, seq, *how).await?;
            }
        }
        for s in &steps {
            if s.how != How::Trash {
                self.db
                    .relocate_model(s.model, &s.from_rel, s.to_lib, &s.to_rel, &s.new_name)
                    .await?;
                if self.db.library(s.to_lib).await?.sidecar_enabled {
                    self.sidecars.queue(s.model);
                }
            }
        }
        self.db
            .set_op_state(
                op,
                if reverse {
                    OpState::Undone
                } else {
                    OpState::Done
                },
            )
            .await?;
        drop(guards);

        if kind == OpKind::Trash {
            self.after_trash(&steps).await?;
        }
        let summary = OpSummary {
            id: op,
            kind,
            state: OpState::Done,
            names: steps.iter().map(|s| s.new_name.clone()).collect(),
            undoable: !reverse && (kind != OpKind::Trash || CAN_RESTORE),
        };
        if !reverse {
            let _ = self.events.send(EngineEvent::FileOpDone(summary.clone()));
        }
        let _ = self.events.send(EngineEvent::OrganizeChanged);
        Ok(summary)
    }

    /// After trashing: the scan marks the models missing; those without
    /// user data are then forgotten.
    async fn after_trash(&self, steps: &[Step]) -> Result<()> {
        let mut by_lib: BTreeMap<LibraryId, Vec<String>> = BTreeMap::new();
        for s in steps {
            by_lib
                .entry(s.from_lib)
                .or_default()
                .push(s.from_rel.clone());
        }
        for (lib, paths) in by_lib {
            self.rescan_paths(lib, paths, tokio_util::sync::CancellationToken::new())
                .await?;
        }
        for s in steps {
            if !self.db.has_user_data(s.model).await? {
                self.db.forget_model(s.model).await?;
                let _ = self.events.send(EngineEvent::ModelForgotten(s.model));
            }
        }
        Ok(())
    }

    /// Puts the files of a trash action back and rescans them.
    async fn restore_trashed(&self, row: &OpRow) -> Result<()> {
        let roots = self.roots(&row.steps).await?;
        let paths: Vec<PathBuf> = row
            .steps
            .iter()
            .flat_map(|s| {
                let root = &roots[&s.from_lib];
                let mut v = vec![join_rel(root, &s.from_rel)];
                if s.kind == ModelKind::SingleFile {
                    v.push(sidecar::path_for(root, &s.from_rel, s.kind));
                }
                v
            })
            .collect();
        tokio::task::spawn_blocking(move || restore(&paths))
            .await
            .map_err(|_| CoreError::TrashUnavailable)??;
        let mut by_lib: BTreeMap<LibraryId, Vec<String>> = BTreeMap::new();
        for s in &row.steps {
            by_lib
                .entry(s.from_lib)
                .or_default()
                .push(s.from_rel.clone());
        }
        for (lib, paths) in by_lib {
            self.rescan_paths(lib, paths, tokio_util::sync::CancellationToken::new())
                .await?;
        }
        Ok(())
    }
}

/// Restores the newest trashed copy of each of `paths`.
#[cfg(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore(paths: &[PathBuf]) -> Result<()> {
    let items = trash::os_limited::list().map_err(|_| CoreError::TrashUnavailable)?;
    let mut pick = Vec::new();
    for p in paths {
        if let Some(item) = items
            .iter()
            .filter(|i| &i.original_path() == p)
            .max_by_key(|i| i.time_deleted)
        {
            pick.push(item.clone());
        }
    }
    if pick.is_empty() {
        return Err(CoreError::UndoUnavailable);
    }
    trash::os_limited::restore_all(pick).map_err(|e| match e {
        trash::Error::RestoreCollision { path, .. } => CoreError::NameTaken(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
        _ => CoreError::UndoUnavailable,
    })
}

#[cfg(not(any(
    target_os = "windows",
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
)))]
fn restore(_paths: &[PathBuf]) -> Result<()> {
    Err(CoreError::UndoUnavailable)
}

/// Why a step failed: the path and the error, and, when the model could
/// not be put back, how it now sits at its target.
struct StepFailure {
    path: PathBuf,
    err: io::Error,
    stuck: Option<How>,
}

impl From<(PathBuf, io::Error)> for StepFailure {
    fn from((path, err): (PathBuf, io::Error)) -> StepFailure {
        StepFailure {
            path,
            err,
            stuck: None,
        }
    }
}

/// The error for a failed step: a taken name, links that can't be copied
/// between drives, or the I/O error.
fn step_error(path: &Path, e: io::Error, s: &Step) -> CoreError {
    match e.kind() {
        io::ErrorKind::AlreadyExists => CoreError::NameTaken(path.file_name().map_or_else(
            || base(&s.to_rel).to_owned(),
            |n| n.to_string_lossy().into_owned(),
        )),
        _ if fsx::is_links_error(&e) => CoreError::HasLinks(path.to_path_buf()),
        _ => io_err(path, e),
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Does one step on disk. Returns how it was done. On failure the step is
/// undone, unless putting it back failed too (`stuck`).
fn do_step(
    from_root: &Path,
    to_root: &Path,
    s: &Step,
    journal_copy: &dyn Fn() -> io::Result<()>,
) -> Result<How, StepFailure> {
    let from = join_rel(from_root, &s.from_rel);
    let side_from = (s.kind == ModelKind::SingleFile)
        .then(|| sidecar::path_for(from_root, &s.from_rel, s.kind))
        .filter(|p| p.exists());
    if s.how == How::Trash {
        let mut all = vec![from.clone()];
        all.extend(side_from);
        // Checked one by one, then trashed together; a test's `Instead`
        // takes a path out of the batch.
        let mut real = Vec::new();
        for p in &all {
            faults::perform(Op::Trash, p, || {
                real.push(p.clone());
                Ok(())
            })
            .map_err(|e| (p.clone(), e))?;
        }
        if !real.is_empty() {
            trash::delete_all(&real)
                .map_err(|e| (from.clone(), io::Error::other(e.to_string())))?;
        }
        return Ok(How::Trash);
    }
    let to = join_rel(to_root, &s.to_rel);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| (parent.to_path_buf(), e))?;
    }
    let how = move_path(&from, &to, journal_copy)?;
    if let Some(side) = side_from {
        let side_to = sidecar::path_for(to_root, &s.to_rel, s.kind);
        // The sidecar stays next to its model: if it can't follow, the
        // model goes back.
        if let Err((path, err)) = move_path(&side, &side_to, &no_journal) {
            let stuck = match put_back(&from, &to, how) {
                Ok(()) => None,
                Err(e) => {
                    tracing::error!(from = %to.display(), to = %from.display(), error = %e.1, "could not put a model back after its sidecar failed to move");
                    Some(how)
                }
            };
            return Err(StepFailure { path, err, stuck });
        }
    }
    Ok(how)
}

fn no_journal() -> io::Result<()> {
    Ok(())
}

/// Moves `from` to `to` without ever replacing anything: a no-replace
/// rename, or, when they are on different drives, a verified copy and then
/// `from` to the trash. If trashing fails the copy is removed again.
/// `journal_copy` runs before a copy starts; if it fails nothing is copied.
fn move_path(
    from: &Path,
    to: &Path,
    journal_copy: &dyn Fn() -> io::Result<()>,
) -> Result<How, (PathBuf, io::Error)> {
    match fsx::rename_no_replace(from, to) {
        Ok(()) => Ok(How::Rename),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            journal_copy().map_err(|e| (from.to_path_buf(), e))?;
            // On failure the copy removes only what it created.
            fsx::copy_tree_verified(from, to).map_err(|e| {
                let at = if fsx::is_links_error(&e) { from } else { to };
                (at.to_path_buf(), e)
            })?;
            if let Err(e) = trash_path(from) {
                if let Err(rm) = remove_path(to) {
                    tracing::error!(path = %to.display(), error = %rm, "could not remove a copy after the original could not be trashed; it is in both places");
                }
                return Err((from.to_path_buf(), e));
            }
            Ok(How::Copy)
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err((to.to_path_buf(), e)),
        Err(e) => Err((from.to_path_buf(), e)),
    }
}

/// Puts a model moved `from` → `to` (done `how`) back at `from`, never
/// replacing anything there.
fn put_back(from: &Path, to: &Path, how: How) -> Result<(), (PathBuf, io::Error)> {
    match how {
        How::Rename => move_path(to, from, &no_journal).map(|_| ()),
        // `to` is our verified copy; the original is in the trash.
        How::Copy => {
            fsx::copy_tree_verified(to, from).map_err(|e| (from.to_path_buf(), e))?;
            if let Err(e) = remove_path(to) {
                // Keep it in one place. If `to` is still whole, drop the copy
                // just made instead; if the removal got partway, `from` is
                // the only complete copy and stays.
                if matches!(fsx::copy_state(from, to), Ok(fsx::CopyState::Complete)) {
                    if let Err(rm) = remove_path(from) {
                        tracing::error!(path = %from.display(), error = %rm, "could not remove a copy; it is in both places");
                    }
                    return Err((to.to_path_buf(), e));
                }
                tracing::error!(path = %to.display(), error = %e, "put a model back, but part of its copy is left behind");
            }
            Ok(())
        }
        How::Trash => restore(&[from.to_path_buf()])
            .map_err(|e| (from.to_path_buf(), io::Error::other(e.to_string()))),
    }
}

/// Reverses a finished step (rolling back a failed action). Returns false
/// when the model could not be put back and stays at its target, with its
/// sidecar.
fn undo_step(from_root: &Path, to_root: &Path, s: &Step, how: How) -> bool {
    let from = join_rel(from_root, &s.from_rel);
    let to = join_rel(to_root, &s.to_rel);
    let side = (s.kind == ModelKind::SingleFile && how != How::Trash)
        .then(|| {
            (
                sidecar::path_for(to_root, &s.to_rel, s.kind),
                sidecar::path_for(from_root, &s.from_rel, s.kind),
            )
        })
        .filter(|(side_to, _)| side_to.exists());
    // The sidecar first: if it can't go back, neither does the model.
    if let Some((side_to, side_from)) = &side
        && let Err(e) = move_path(side_to, side_from, &no_journal)
    {
        tracing::error!(from = %side_to.display(), to = %side_from.display(), error = %e.1, "could not roll back a sidecar");
        return false;
    }
    if let Err(e) = put_back(&from, &to, how) {
        tracing::error!(from = %to.display(), to = %from.display(), error = %e.1, "could not roll back a file action step");
        if let Some((side_to, side_from)) = &side
            && let Err(e) = move_path(side_from, side_to, &no_journal)
        {
            tracing::error!(path = %side_from.display(), error = %e.1, "sidecar left apart from its model");
        }
        return false;
    }
    true
}

/// After a crash, settles a rename or copy step from what the disk shows:
/// returns whether the model is at its target. A copy left at the target
/// next to an intact original (or a copy back cut short next to a whole
/// target) is removed, but only when it is provably a copy of the other;
/// the only complete copy is never removed. A single file's sidecar is
/// settled the same way, keeping the one next to the model. Blocking.
fn settle_step(from: &Path, to: &Path, how: How, sides: Option<(PathBuf, PathBuf)>) -> bool {
    let at_target = match (from.exists(), to.exists()) {
        (false, true) => true,
        (true, true) if how == How::Copy => drop_copy(from, to) == Some(false),
        _ => false,
    };
    if let Some((side_from, side_to)) = sides
        && side_from.exists()
        && side_to.exists()
    {
        let (keep, other) = if at_target {
            (side_to, side_from)
        } else {
            (side_from, side_to)
        };
        drop_copy(&keep, &other);
    }
    at_target
}

/// With both `a` and `b` present: removes `b` if it is a whole or partial
/// copy of `a` (`Some(true)`, `a` kept), else `a` if it is a partial copy of
/// `b` (`Some(false)`, `b` kept). Leaves both when neither is (`None`).
fn drop_copy(a: &Path, b: &Path) -> Option<bool> {
    use fsx::CopyState::{Complete, Partial};
    let (keep_a, drop) = match (fsx::copy_state(a, b), fsx::copy_state(b, a)) {
        (Ok(Complete | Partial), _) => (true, b),
        (_, Ok(Partial)) => (false, a),
        _ => {
            tracing::warn!(a = %a.display(), b = %b.display(), "after a crash both copies are there and neither is a copy of the other; left alone");
            return None;
        }
    };
    match remove_path(drop) {
        Ok(()) => Some(keep_a),
        Err(e) => {
            tracing::warn!(path = %drop.display(), error = %e, "could not remove an unfinished copy");
            None
        }
    }
}

fn trash_path(p: &Path) -> io::Result<()> {
    faults::perform(Op::Trash, p, || {
        trash::delete(p).map_err(|e| io::Error::other(e.to_string()))
    })
}

fn remove_path(p: &Path) -> io::Result<()> {
    faults::perform(Op::Remove, p, || {
        if std::fs::symlink_metadata(p)?.is_dir() {
            std::fs::remove_dir_all(p)
        } else {
            std::fs::remove_file(p)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_work_everywhere() {
        for ok in ["Wall hook", "Dragon v2.stl", "Ätzend", "a.b.c"] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            ".",
            "..",
            ".hidden",
            "a/b",
            "a\\b",
            "a:b",
            "what?",
            "trail.",
            "trail ",
            " lead",
            "CON",
            "com1.stl",
            "tab\there",
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn path_helpers() {
        assert_eq!(parent_rel("a/b/c"), "a/b");
        assert_eq!(parent_rel("c"), "");
        assert_eq!(base("a/b/c.stl"), "c.stl");
        assert_eq!(child("", "x"), "x");
        assert_eq!(child("a", "x"), "a/x");
        assert!(inside("a/b", "a"));
        assert!(inside("a", "a"));
        assert!(!inside("ab", "a"));
        assert_eq!(model_name("hook.stl", ModelKind::SingleFile), "hook");
        assert_eq!(model_name("Hook v2", ModelKind::Folder), "Hook v2");
    }
}
