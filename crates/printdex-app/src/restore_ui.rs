//! Restoring from a backup: the "Restore from backup" wizard (target,
//! point in time, restart), the per-library "where does it live now?"
//! question after a restore, and restoring one model.

use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use printdex_core::backup::{FileChange, FileDiff, ModelRestoreDest, RestoredData, SnapshotInfo};
use printdex_core::{CoreError, Library, LibraryId, ModelId};

use crate::app::{App, Message, Screen, SettingsSection};
use crate::backup_ui::{Field, Form, Kind};
use crate::i18n::t;

/// Where the restore wizard is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Entering the target and passphrase.
    Target,
    Finding,
    /// Picking a point in time.
    Pick,
    Restoring,
    /// Restored; waiting for the restart.
    Done,
}

/// The "Restore from backup" wizard.
#[derive(Clone)]
pub struct Wizard {
    pub form: Form,
    pub step: Step,
    /// Newest first.
    pub snapshots: Vec<SnapshotInfo>,
    pub picked: Option<String>,
    pub error: Option<String>,
}

impl Wizard {
    pub fn new() -> Wizard {
        let mut form = Form::new(&[]);
        // Folders are often unencrypted, remote targets usually not; the
        // passphrase field shows either way once encryption is on.
        form.set_kind(Kind::Folder);
        Wizard {
            form,
            step: Step::Target,
            snapshots: Vec::new(),
            picked: None,
            error: None,
        }
    }

    /// Whether "Find backups" can run.
    pub fn can_find(&self) -> bool {
        self.step == Step::Target
            && self.form.target().is_some()
            && self.form.test_blocker().is_none()
            && (!self.form.encrypted || !self.form.passphrase.is_empty())
    }
}

/// What a library can do after a data restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relink {
    /// "It's already here": the folder (or a folder the user picks).
    Here,
    /// "Restore files from backup": only when the library has model
    /// snapshots.
    FromBackup,
    /// "Skip for now".
    Skip,
}

/// The choices offered for a library.
pub fn relink_choices(has_snapshot: bool) -> Vec<Relink> {
    let mut v = vec![Relink::Here];
    if has_snapshot {
        v.push(Relink::FromBackup);
    }
    v.push(Relink::Skip);
    v
}

/// A library waiting for "where does it live now?", with its newest model
/// snapshot (if any).
#[derive(Clone, Debug)]
pub struct Pending {
    pub library: Library,
    pub snapshot: Option<SnapshotInfo>,
}

/// "Restore from backup…" for one model.
#[derive(Clone, Debug, Default)]
pub struct ModelRestore {
    pub model: Option<ModelId>,
    /// Newest first; `None` while loading.
    pub snapshots: Option<Vec<SnapshotInfo>>,
    pub picked: Option<String>,
    /// `None` while comparing.
    pub preview: Option<Vec<FileDiff>>,
    pub busy: bool,
    pub error: Option<String>,
    /// Files written by the last restore.
    pub done: Option<usize>,
}

/// The files a restore would write: changed and missing ones.
pub fn to_restore(preview: &[FileDiff]) -> Vec<&FileDiff> {
    preview
        .iter()
        .filter(|d| d.change != FileChange::Same)
        .collect()
}

#[derive(Default)]
pub struct RestoreUi {
    pub wizard: Option<Wizard>,
    pub pending: Vec<Pending>,
    /// The library whose relink or file restore runs.
    pub busy: Option<LibraryId>,
    pub model: ModelRestore,
}

#[derive(Clone, Debug)]
pub enum Rx {
    Start,
    Cancel,
    Kind(Kind),
    Input(Field, String),
    Encrypted(bool),
    Remember(bool),
    PickFolder,
    FolderPicked(Option<PathBuf>),
    Find,
    Found(Result<Vec<SnapshotInfo>, Arc<CoreError>>),
    Pick(String),
    Back,
    Restore,
    Restored(Result<RestoredData, Arc<CoreError>>),
    Restart,
    Pending(Result<Vec<Pending>, Arc<CoreError>>),
    Here(LibraryId),
    HerePicked(LibraryId, Option<PathBuf>),
    FromBackup(LibraryId),
    FromBackupPicked(LibraryId, Option<PathBuf>),
    Skip(LibraryId),
    Relinked(LibraryId, Result<(), Arc<CoreError>>),
    OpenModel(ModelId),
    CloseModel,
    ModelSnapshots(ModelId, Result<Vec<SnapshotInfo>, Arc<CoreError>>),
    PickModel(String),
    Preview(String, Result<Vec<FileDiff>, Arc<CoreError>>),
    RestoreHere,
    RestoreElsewhere,
    ElsewherePicked(Option<PathBuf>),
    ModelRestored(bool, Result<Vec<FileDiff>, Arc<CoreError>>),
}

fn err_text(e: &CoreError) -> String {
    match e {
        CoreError::Backup(b) => crate::backup_ui::backup_error_text(b),
        other => crate::app::error_text(other),
    }
}

async fn pick_folder(title: String, start: Option<PathBuf>) -> Option<PathBuf> {
    let mut d = rfd::AsyncFileDialog::new().set_title(title);
    if let Some(s) = start.filter(|s| s.is_dir()) {
        d = d.set_directory(s);
    }
    d.pick_folder().await.map(|h| h.path().to_path_buf())
}

/// Starts this program again with the same arguments, then quits.
fn restart() -> Task<Message> {
    match std::env::current_exe() {
        Ok(exe) => {
            if let Err(e) = std::process::Command::new(exe)
                .args(std::env::args_os().skip(1))
                .spawn()
            {
                tracing::error!(error = %e, "could not restart");
            }
        }
        Err(e) => tracing::error!(error = %e, "could not find the program to restart"),
    }
    iced::exit()
}

impl App {
    /// Whether the empty library offers "Restore from backup" (first
    /// launch: no libraries, no backup set up).
    pub fn offers_restore(&self) -> bool {
        self.libraries.is_empty() && self.search.loaded && self.backup.target().is_none()
    }

    /// Loads the libraries waiting for "where does it live now?".
    pub(crate) fn load_pending(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let libs = engine.pending_relinks().await?;
                let mut out = Vec::new();
                for library in libs {
                    // No snapshots (or no secrets yet) just hides the
                    // "Restore files" choice.
                    let snapshot = engine
                        .library_snapshots(library.id)
                        .await
                        .ok()
                        .and_then(|s| s.into_iter().next());
                    out.push(Pending { library, snapshot });
                }
                Ok(out)
            },
            |r: Result<_, CoreError>| Message::Restore(Rx::Pending(r.map_err(Arc::new))),
        )
    }

    fn relink(&mut self, id: LibraryId, path: PathBuf, from: Option<String>) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        self.restore.busy = Some(id);
        Task::perform(
            async move {
                match from {
                    Some(snap) => engine.restore_library_files(id, &snap, &path).await,
                    None => engine.relink_library(id, &path).await,
                }
                .map(|_| ())
                .map_err(Arc::new)
            },
            move |r| Message::Restore(Rx::Relinked(id, r)),
        )
    }

    fn preview_model(&mut self, snapshot: String) -> Task<Message> {
        let (Some(engine), Some(model)) = (self.engine.clone(), self.restore.model.model) else {
            return Task::none();
        };
        let m = &mut self.restore.model;
        m.picked = Some(snapshot.clone());
        m.preview = None;
        m.done = None;
        m.error = None;
        Task::perform(
            async move {
                let r = engine
                    .model_restore_preview(model, &snapshot)
                    .await
                    .map_err(Arc::new);
                (snapshot, r)
            },
            |(s, r)| Message::Restore(Rx::Preview(s, r)),
        )
    }

    fn restore_model_to(&mut self, dest: ModelRestoreDest) -> Task<Message> {
        let m = &mut self.restore.model;
        let (Some(engine), Some(model), Some(snap)) =
            (self.engine.clone(), m.model, m.picked.clone())
        else {
            return Task::none();
        };
        m.busy = true;
        m.error = None;
        let original = dest == ModelRestoreDest::Original;
        Task::perform(
            async move {
                engine
                    .restore_model(model, &snap, dest)
                    .await
                    .map_err(Arc::new)
            },
            move |r| Message::Restore(Rx::ModelRestored(original, r)),
        )
    }

    fn wizard(&mut self) -> Option<&mut Wizard> {
        self.restore.wizard.as_mut()
    }

    pub fn on_restore(&mut self, msg: Rx) -> Task<Message> {
        match msg {
            Rx::Start => {
                self.restore.wizard = Some(Wizard::new());
                self.screen = Screen::Settings;
                self.settings_section = SettingsSection::Backup;
                self.load_backup()
            }
            Rx::Cancel => {
                self.restore.wizard = None;
                Task::none()
            }
            Rx::Kind(k) => {
                if let Some(w) = self.wizard() {
                    w.form.set_kind(k);
                }
                Task::none()
            }
            Rx::Input(f, v) => {
                if let Some(w) = self.wizard() {
                    w.form.set(f, v);
                }
                Task::none()
            }
            Rx::Encrypted(on) => {
                if let Some(w) = self.wizard() {
                    w.form.encrypted = on;
                }
                Task::none()
            }
            Rx::Remember(on) => {
                if let Some(w) = self.wizard() {
                    w.form.remember = on;
                }
                Task::none()
            }
            Rx::PickFolder => Task::perform(pick_folder(t("backup-pick-folder"), None), |p| {
                Message::Restore(Rx::FolderPicked(p))
            }),
            Rx::FolderPicked(Some(p)) => {
                if let Some(w) = self.wizard() {
                    w.form.set(Field::Path, p.display().to_string());
                }
                Task::none()
            }
            Rx::FolderPicked(None) => Task::none(),
            Rx::Find => {
                let engine = self.engine.clone();
                let Some(w) = self.wizard() else {
                    return Task::none();
                };
                let (Some(engine), Some(target)) = (engine, w.form.target()) else {
                    return Task::none();
                };
                if !w.can_find() {
                    return Task::none();
                }
                w.step = Step::Finding;
                w.error = None;
                let secrets = w.form.secrets();
                Task::perform(
                    async move {
                        engine
                            .data_snapshots(&target, &secrets)
                            .await
                            .map_err(Arc::new)
                    },
                    |r| Message::Restore(Rx::Found(r)),
                )
            }
            Rx::Found(r) => {
                if let Some(w) = self.wizard() {
                    match r {
                        Ok(snaps) if snaps.is_empty() => {
                            w.step = Step::Target;
                            w.error = Some(t("restore-none-found"));
                        }
                        Ok(snaps) => {
                            w.picked = snaps.first().map(|s| s.id.clone());
                            w.snapshots = snaps;
                            w.step = Step::Pick;
                        }
                        Err(e) => {
                            w.step = Step::Target;
                            w.error = Some(err_text(&e));
                        }
                    }
                }
                Task::none()
            }
            Rx::Pick(id) => {
                if let Some(w) = self.wizard() {
                    w.picked = Some(id);
                }
                Task::none()
            }
            Rx::Back => {
                if let Some(w) = self.wizard() {
                    w.step = Step::Target;
                    w.error = None;
                }
                Task::none()
            }
            Rx::Restore => {
                let engine = self.engine.clone();
                let Some(w) = self.wizard() else {
                    return Task::none();
                };
                let (Some(engine), Some(target), Some(snap)) =
                    (engine, w.form.target(), w.picked.clone())
                else {
                    return Task::none();
                };
                w.step = Step::Restoring;
                w.error = None;
                let (secrets, remember) = (w.form.secrets(), w.form.remember);
                Task::perform(
                    async move {
                        engine
                            .restore_data(&target, &secrets, &snap, remember)
                            .await
                            .map_err(Arc::new)
                    },
                    |r| Message::Restore(Rx::Restored(r)),
                )
            }
            Rx::Restored(Ok(data)) => {
                // Settings come back with the data; they take effect after
                // the restart.
                if let Some(dir) = &self.options.config_dir {
                    for f in &data.settings_files {
                        if let Some(name) = f.file_name()
                            && let Err(e) = std::fs::copy(f, dir.join(name))
                        {
                            tracing::warn!(error = %e, "could not restore a settings file");
                        }
                    }
                }
                if let Some(w) = self.wizard() {
                    w.step = Step::Done;
                }
                Task::none()
            }
            Rx::Restored(Err(e)) => {
                if let Some(w) = self.wizard() {
                    w.step = Step::Pick;
                    w.error = Some(err_text(&e));
                }
                Task::none()
            }
            Rx::Restart => restart(),
            Rx::Pending(Ok(p)) => {
                self.restore.pending = p;
                Task::none()
            }
            Rx::Pending(Err(e)) => {
                tracing::warn!(error = %e, "could not list libraries to relink");
                Task::none()
            }
            Rx::Here(id) => {
                let Some(lib) = self.library(id).cloned() else {
                    return Task::none();
                };
                if lib.path.is_dir() {
                    self.relink(id, lib.path, None)
                } else {
                    Task::perform(
                        pick_folder(
                            t_lib("restore-where-pick", &lib.display_name),
                            lib.path.parent().map(PathBuf::from),
                        ),
                        move |p| Message::Restore(Rx::HerePicked(id, p)),
                    )
                }
            }
            Rx::HerePicked(id, Some(p)) => self.relink(id, p, None),
            Rx::FromBackup(id) => {
                let name = self
                    .library(id)
                    .map(|l| l.display_name.clone())
                    .unwrap_or_default();
                Task::perform(
                    pick_folder(t_lib("restore-files-pick", &name), None),
                    move |p| Message::Restore(Rx::FromBackupPicked(id, p)),
                )
            }
            Rx::FromBackupPicked(id, Some(p)) => {
                let snap = self
                    .restore
                    .pending
                    .iter()
                    .find(|x| x.library.id == id)
                    .and_then(|x| x.snapshot.as_ref())
                    .map(|s| s.id.clone());
                match snap {
                    Some(s) => self.relink(id, p, Some(s)),
                    None => Task::none(),
                }
            }
            Rx::HerePicked(_, None) | Rx::FromBackupPicked(_, None) => Task::none(),
            Rx::Skip(id) => {
                self.restore.pending.retain(|x| x.library.id != id);
                match self.engine.clone() {
                    Some(engine) => Task::perform(
                        async move { engine.skip_relink(id).await.map_err(Arc::new) },
                        move |r| Message::Restore(Rx::Relinked(id, r)),
                    ),
                    None => Task::none(),
                }
            }
            Rx::Relinked(id, r) => {
                self.restore.busy = None;
                match r {
                    Ok(()) => {
                        self.restore.pending.retain(|x| x.library.id != id);
                        self.load_libraries()
                    }
                    Err(e) => {
                        self.error = Some(err_text(&e));
                        Task::none()
                    }
                }
            }
            Rx::OpenModel(model) => {
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                self.restore.model = ModelRestore {
                    model: Some(model),
                    ..ModelRestore::default()
                };
                Task::perform(
                    async move { engine.model_snapshots(model).await.map_err(Arc::new) },
                    move |r| Message::Restore(Rx::ModelSnapshots(model, r)),
                )
            }
            Rx::CloseModel => {
                self.restore.model = ModelRestore::default();
                Task::none()
            }
            Rx::ModelSnapshots(model, r) => {
                if self.restore.model.model != Some(model) {
                    return Task::none();
                }
                match r {
                    Ok(snaps) => {
                        let first = snaps.first().map(|s| s.id.clone());
                        self.restore.model.snapshots = Some(snaps);
                        match first {
                            Some(id) => self.preview_model(id),
                            None => Task::none(),
                        }
                    }
                    Err(e) => {
                        self.restore.model.snapshots = Some(Vec::new());
                        self.restore.model.error = Some(err_text(&e));
                        Task::none()
                    }
                }
            }
            Rx::PickModel(id) => self.preview_model(id),
            Rx::Preview(snap, r) => {
                let m = &mut self.restore.model;
                if m.picked.as_deref() != Some(snap.as_str()) {
                    return Task::none();
                }
                match r {
                    Ok(p) => m.preview = Some(p),
                    Err(e) => {
                        m.preview = Some(Vec::new());
                        m.error = Some(err_text(&e));
                    }
                }
                Task::none()
            }
            Rx::RestoreHere => self.restore_model_to(ModelRestoreDest::Original),
            Rx::RestoreElsewhere => {
                Task::perform(pick_folder(t("restore-model-pick"), None), |p| {
                    Message::Restore(Rx::ElsewherePicked(p))
                })
            }
            Rx::ElsewherePicked(Some(p)) => self.restore_model_to(ModelRestoreDest::Folder(p)),
            Rx::ElsewherePicked(None) => Task::none(),
            Rx::ModelRestored(original, r) => {
                let m = &mut self.restore.model;
                m.busy = false;
                match r {
                    Ok(written) => {
                        m.done = Some(written.len());
                        if !original {
                            return Task::none();
                        }
                        // The written files now match the snapshot.
                        if let Some(p) = &mut m.preview {
                            for d in p.iter_mut() {
                                if written.iter().any(|w| w.rel == d.rel) {
                                    d.change = FileChange::Same;
                                }
                            }
                        }
                        let id = m.model;
                        id.map_or(Task::none(), |id| self.load_model(id))
                    }
                    Err(e) => {
                        m.error = Some(err_text(&e));
                        Task::none()
                    }
                }
            }
        }
    }
}

fn t_lib(key: &str, name: &str) -> String {
    crate::i18n::t_args(key, &[("library", crate::i18n::Arg::str(name.to_owned()))])
}

#[cfg(test)]
mod tests {
    use super::*;
    use printdex_core::backup::{BackupConfig, Target, TargetConfig};

    fn app() -> App {
        App::new(crate::app::Options::in_memory(PathBuf::from(
            "/nonexistent",
        )))
    }

    fn lib(name: &str) -> Library {
        Library {
            id: LibraryId::new(),
            path: PathBuf::from(format!("/nonexistent/{name}")),
            display_name: name.into(),
            kind: printdex_core::LibraryKind::Local,
            change_mode: printdex_core::ChangeMode::Auto,
            sidecar_enabled: true,
            last_full_scan_at: None,
        }
    }

    #[test]
    fn first_launch_offers_restore() {
        let mut a = app();
        a.search.loaded = true;
        assert!(a.offers_restore());
        let _ = a.on_restore(Rx::Start);
        assert!(a.restore.wizard.is_some());
        assert_eq!(a.screen, Screen::Settings);
        assert_eq!(a.settings_section, SettingsSection::Backup);
        // Finding needs a target first, and the passphrase when encrypted.
        let w = a.restore.wizard.as_mut().unwrap();
        assert!(!w.can_find());
        w.form.set(Field::Path, "/media/usb/printdex".into());
        assert!(w.can_find());
        w.form.encrypted = true;
        assert!(!w.can_find());
        w.form.passphrase = "x".into();
        assert!(w.can_find());

        // Not once there is a library or a backup.
        let mut b = app();
        b.search.loaded = true;
        b.libraries.push(lib("Prints"));
        assert!(!b.offers_restore());
        let mut c = app();
        c.search.loaded = true;
        c.backup.config = Some(BackupConfig {
            targets: vec![TargetConfig {
                id: 1,
                target: Target::Folder { path: "/b".into() },
                encrypted: false,
            }],
            ..BackupConfig::default()
        });
        assert!(!c.offers_restore());
    }

    #[test]
    fn library_relink_choices() {
        assert_eq!(relink_choices(false), [Relink::Here, Relink::Skip]);
        assert_eq!(
            relink_choices(true),
            [Relink::Here, Relink::FromBackup, Relink::Skip]
        );
        let mut a = app();
        let (l1, l2) = (lib("Prints"), lib("NAS"));
        a.restore.pending = vec![
            Pending {
                library: l1.clone(),
                snapshot: None,
            },
            Pending {
                library: l2.clone(),
                snapshot: None,
            },
        ];
        let _ = a.on_restore(Rx::Skip(l1.id));
        assert_eq!(a.restore.pending.len(), 1);
        assert_eq!(a.restore.pending[0].library.id, l2.id);
        // A finished relink removes the question too.
        let _ = a.on_restore(Rx::Relinked(l2.id, Ok(())));
        assert!(a.restore.pending.is_empty());
    }

    #[test]
    fn model_restore_preview_lists_changed_files() {
        let mut a = app();
        let model = ModelId::new();
        a.restore.model = ModelRestore {
            model: Some(model),
            picked: Some("s1".into()),
            ..ModelRestore::default()
        };
        let preview = vec![
            FileDiff {
                rel: "a.stl".into(),
                change: FileChange::Changed,
            },
            FileDiff {
                rel: "b.stl".into(),
                change: FileChange::Same,
            },
            FileDiff {
                rel: "c.stl".into(),
                change: FileChange::Missing,
            },
        ];
        // A stale preview (another snapshot) is ignored.
        let _ = a.on_restore(Rx::Preview("s0".into(), Ok(preview.clone())));
        assert!(a.restore.model.preview.is_none());
        let _ = a.on_restore(Rx::Preview("s1".into(), Ok(preview)));
        let shown = to_restore(a.restore.model.preview.as_ref().unwrap());
        let names: Vec<_> = shown.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(names, ["a.stl", "c.stl"]);
    }
}
