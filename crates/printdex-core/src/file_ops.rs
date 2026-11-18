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
            if steps.iter().any(|s: &Step| s.to_rel == to_rel) {
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
                        if to.exists() && !from.exists() {
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
                        } else if to.exists() && from.exists() && s.how == How::Copy {
                            // A copy that never finished: the original stays.
                            let _ = remove_any(&to);
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
            let case_only = s.from_lib == s.to_lib && s.from_rel.eq_ignore_ascii_case(&s.to_rel);
            let taken = (!case_only
                && self
                    .fs
                    .metadata(&join_rel(&roots[&s.to_lib], &s.to_rel))
                    .is_ok())
                || self.db.path_taken(s.to_lib, &s.to_rel, s.model).await?;
            if taken {
                return Err(CoreError::NameTaken(base(&s.to_rel).to_owned()));
            }
        }

        let op = self.db.insert_op(kind, &steps).await?;
        let work = steps.clone();
        let work_roots = roots.clone();
        let (done, failure) = tokio::task::spawn_blocking(move || {
            let mut done: Vec<How> = Vec::new();
            for s in &work {
                match do_step(&work_roots[&s.from_lib], &work_roots[&s.to_lib], s) {
                    Ok(how) => done.push(how),
                    Err(e) => {
                        // Put back what was already done, newest first.
                        for (s, how) in work.iter().zip(&done).rev() {
                            undo_step(&work_roots[&s.from_lib], &work_roots[&s.to_lib], s, *how);
                        }
                        return (done, Some(e));
                    }
                }
            }
            (done, None)
        })
        .await
        .map_err(|e| io_err(&roots[&libs[0]], io::Error::other(e.to_string())))?;
        if let Some((path, e)) = failure {
            self.db.set_op_state(op, OpState::Failed).await?;
            tracing::warn!(path = %path.display(), error = %e, "file action failed; undone");
            return Err(io_err(&path, e));
        }
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

/// Does one step on disk. Returns how it was done, or the path that failed.
fn do_step(from_root: &Path, to_root: &Path, s: &Step) -> Result<How, (PathBuf, io::Error)> {
    let from = join_rel(from_root, &s.from_rel);
    let side_from = (s.kind == ModelKind::SingleFile)
        .then(|| sidecar::path_for(from_root, &s.from_rel, s.kind))
        .filter(|p| p.exists());
    if s.how == How::Trash {
        let mut all = vec![from.clone()];
        all.extend(side_from);
        trash::delete_all(&all).map_err(|e| (from.clone(), io::Error::other(e.to_string())))?;
        return Ok(How::Trash);
    }
    let to = join_rel(to_root, &s.to_rel);
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| (parent.to_path_buf(), e))?;
    }
    let how = move_path(&from, &to)?;
    if let Some(side) = side_from {
        let side_to = sidecar::path_for(to_root, &s.to_rel, s.kind);
        // Printdex's own small file: a failure here is not worth undoing
        // the move for; the writer makes a new one.
        if let Err(e) = move_path(&side, &side_to) {
            tracing::warn!(error = %e.1, "could not move sidecar");
        }
    }
    Ok(how)
}

/// Renames `from` to `to`, or copies, checks and trashes `from` when they
/// are on different drives.
fn move_path(from: &Path, to: &Path) -> Result<How, (PathBuf, io::Error)> {
    match std::fs::rename(from, to) {
        Ok(()) => Ok(How::Rename),
        Err(e) if e.kind() == io::ErrorKind::CrossesDevices => {
            copy_tree(from, to).map_err(|e| {
                let _ = remove_any(to);
                (to.to_path_buf(), e)
            })?;
            if tree_listing(from).ok() != tree_listing(to).ok() {
                let _ = remove_any(to);
                return Err((
                    to.to_path_buf(),
                    io::Error::other("copy does not match the original"),
                ));
            }
            if let Err(e) = trash::delete(from) {
                let _ = remove_any(to);
                return Err((from.to_path_buf(), io::Error::other(e.to_string())));
            }
            Ok(How::Copy)
        }
        Err(e) => Err((from.to_path_buf(), e)),
    }
}

/// Reverses a finished step (rolling back a failed action). Best effort.
fn undo_step(from_root: &Path, to_root: &Path, s: &Step, how: How) {
    let from = join_rel(from_root, &s.from_rel);
    let to = join_rel(to_root, &s.to_rel);
    let result = match how {
        How::Rename => std::fs::rename(&to, &from),
        How::Copy => copy_tree(&to, &from).and_then(|()| remove_any(&to)),
        How::Trash => {
            restore(std::slice::from_ref(&from)).map_err(|e| io::Error::other(e.to_string()))
        }
    };
    if let Err(e) = result {
        tracing::error!(from = %to.display(), to = %from.display(), error = %e, "could not roll back a file action step");
    }
    if s.kind == ModelKind::SingleFile && how != How::Trash {
        let side_to = sidecar::path_for(to_root, &s.to_rel, s.kind);
        if side_to.exists() {
            let _ = std::fs::rename(&side_to, sidecar::path_for(from_root, &s.from_rel, s.kind));
        }
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.is_dir() {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else if meta.is_file() {
        std::fs::copy(from, to).map(|_| ())
    } else {
        // Symlinks and specials are not followed or copied.
        Ok(())
    }
}

/// Every regular file under `root` with its size, for comparing a copy.
fn tree_listing(root: &Path) -> io::Result<Vec<(PathBuf, u64)>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(p) = stack.pop() {
        let meta = std::fs::symlink_metadata(&p)?;
        if meta.is_dir() {
            for e in std::fs::read_dir(&p)? {
                stack.push(e?.path());
            }
        } else if meta.is_file() {
            let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
            out.push((rel, meta.len()));
        }
    }
    out.sort();
    Ok(out)
}

fn remove_any(p: &Path) -> io::Result<()> {
    if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    }
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

    #[test]
    fn copy_tree_copies_and_lists_equal() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        std::fs::create_dir_all(a.join("sub")).unwrap();
        std::fs::write(a.join("x.stl"), b"xx").unwrap();
        std::fs::write(a.join("sub/y.stl"), b"y").unwrap();
        copy_tree(&a, &d.path().join("b")).unwrap();
        assert_eq!(
            tree_listing(&a).unwrap(),
            tree_listing(&d.path().join("b")).unwrap()
        );
    }
}
