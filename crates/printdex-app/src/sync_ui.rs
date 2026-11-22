//! Settings › Sync, the sidebar's sync line and its popover, and the
//! "Edited elsewhere" entries and "on Laptop" notes of the model view.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use iced::Task;
use printdex_core::db::{DeviceRow, EditEntry};
use printdex_core::ids::Uuid;
use printdex_core::sync::log::{AttemptField, Key, ModelField, RecipeField};
use printdex_core::sync::{
    FORGET_AFTER_MS, ParkedModel, RemoteLibrary, SpaceInfo, SyncAttribution,
};
use printdex_core::{CoreError, Library, LibraryId, ModelId, SyncError};

use crate::app::{App, Message, Screen, SettingsSection};
use crate::backup_ui::{Field, Form, Kind};
use crate::i18n::{Arg, t, t_args};

/// Where a new space lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    /// `<library>/.printdex/sync`: every computer opens the same library
    /// (a NAS share).
    Library,
    /// A folder, S3, WebDAV or SFTP place: each computer has its own copy.
    Target,
}

/// The "Add space" form.
#[derive(Clone)]
pub struct AddSpace {
    pub place: Place,
    /// The library to sync (required inside a library; linked first for
    /// another place).
    pub library: Option<LibraryId>,
    /// Where and how, for another place; the passphrase fields for both.
    pub form: Form,
    /// The space exists and is encrypted: only its passphrase is asked.
    pub joining: bool,
    pub saving: bool,
    pub error: Option<String>,
}

impl AddSpace {
    /// A fresh form. With a library given (from "Sync this library…"),
    /// the space goes inside it.
    pub fn new(libraries: &[Library], library: Option<LibraryId>) -> AddSpace {
        let mut a = AddSpace {
            place: Place::Library,
            library: library.or_else(|| libraries.first().map(|l| l.id)),
            form: Form::new(&[]),
            joining: false,
            saving: false,
            error: None,
        };
        a.set_place(Place::Library);
        a
    }

    /// Encryption starts off inside a library (the files stay on your own
    /// share) and on for another place.
    pub fn set_place(&mut self, place: Place) {
        self.place = place;
        self.form.encrypted = place == Place::Target;
        self.joining = false;
        self.error = None;
    }

    pub fn set_kind(&mut self, kind: Kind) {
        self.form.set_kind(kind);
        self.form.encrypted = true;
        self.joining = false;
    }

    /// Why the form can't be saved yet (`None`: it can).
    pub fn save_blocker(&self) -> Option<String> {
        if self.place == Place::Library && self.library.is_none() {
            return Some(t("sync-need-library"));
        }
        if self.place == Place::Target {
            if self.form.target().is_none() {
                return Some(t("sync-need-fields"));
            }
            if let Some(b) = self.form.test_blocker() {
                return Some(b);
            }
        }
        if self.form.encrypted {
            if self.form.passphrase.is_empty() {
                return Some(t("backup-need-passphrase"));
            }
            if !self.joining {
                if self.form.passphrase != self.form.passphrase2 {
                    return Some(t("backup-passphrase-mismatch"));
                }
                if !self.form.ack {
                    return Some(t("backup-need-ack"));
                }
            }
        }
        None
    }

    pub fn can_save(&self) -> bool {
        !self.saving && self.save_blocker().is_none()
    }

    fn passphrase(&self) -> String {
        if self.form.encrypted {
            self.form.passphrase.clone()
        } else {
            String::new()
        }
    }
}

/// Linking a local library to one in a space.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkDialog {
    pub space: i64,
    pub library: Option<LibraryId>,
    pub remotes: Vec<RemoteLibrary>,
    /// The space's library to link to; `None`: add it as a new one.
    pub choice: Option<Uuid>,
    pub busy: bool,
}

impl LinkDialog {
    pub fn new(space: i64, library: Option<LibraryId>, remotes: Vec<RemoteLibrary>) -> LinkDialog {
        let mut d = LinkDialog {
            space,
            library: None,
            remotes,
            choice: None,
            busy: false,
        };
        if let Some(lib) = library {
            d.set_library(lib);
        }
        d
    }

    /// Picks the local library, and preselects the space's library that
    /// holds the same models (or "new" when none does).
    pub fn set_library(&mut self, lib: LibraryId) {
        self.library = Some(lib);
        self.choice = self
            .remotes
            .iter()
            .find(|r| r.linked.is_none() && r.suggested == Some(lib))
            .map(|r| r.id);
    }

    /// The space's libraries a local one can still link to.
    pub fn open_remotes(&self) -> impl Iterator<Item = &RemoteLibrary> {
        self.remotes.iter().filter(|r| r.linked.is_none())
    }
}

/// Unlocking an encrypted space whose passphrase isn't stored here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Unlock {
    pub space: i64,
    pub passphrase: String,
    pub remember: bool,
    pub busy: bool,
    pub error: Option<String>,
}

/// The open model's sync extras.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelSync {
    pub id: Option<ModelId>,
    pub history: Vec<EditEntry>,
    pub attribution: SyncAttribution,
}

/// What Settings › Sync loads from the engine.
#[derive(Clone, Debug)]
pub struct Loaded {
    pub enabled: bool,
    pub device: String,
    pub me: Uuid,
    pub spaces: Vec<SpaceInfo>,
    pub queued: usize,
}

#[derive(Default)]
pub struct SyncUi {
    pub loaded: bool,
    pub enabled: bool,
    /// The device name field, and the name as saved.
    pub device: String,
    pub device_saved: String,
    pub me: Option<Uuid>,
    pub spaces: Vec<SpaceInfo>,
    /// Changed models waiting for the next sync.
    pub queued: usize,
    /// Spaces syncing right now.
    pub running: HashSet<i64>,
    pub expanded: HashSet<i64>,
    pub add: Option<AddSpace>,
    pub link: Option<LinkDialog>,
    pub unlock: Option<Unlock>,
    pub parked: Option<(i64, Vec<ParkedModel>)>,
    pub confirm_remove: Option<i64>,
    pub confirm_forget: Option<(i64, Uuid)>,
    pub popover: bool,
    pub error: Option<String>,
    pub model: ModelSync,
}

impl SyncUi {
    pub fn space(&self, id: i64) -> Option<&SpaceInfo> {
        self.spaces.iter().find(|s| s.id == id)
    }

    /// Whether the sidebar shows the sync line.
    pub fn shown(&self) -> bool {
        !self.spaces.is_empty()
    }
}

/// How the sidebar line looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Quiet,
    Busy,
    Attention,
}

/// The sidebar line: "Synced 2 min ago", "Waiting for NAS",
/// "Passphrase needed", "Sync off".
pub fn sync_status_text(enabled: bool, spaces: &[SpaceInfo], now_ms: i64) -> (String, Tone) {
    if !enabled || spaces.is_empty() {
        return (t("sync-status-off"), Tone::Quiet);
    }
    if spaces.iter().any(|s| s.locked) {
        return (t("sync-status-locked"), Tone::Attention);
    }
    if let Some(s) = spaces.iter().find(|s| s.last_error.is_some()) {
        return (
            t_args(
                "sync-status-waiting",
                &[("space", Arg::str(s.name.clone()))],
            ),
            Tone::Attention,
        );
    }
    // The oldest space tells how current everything is.
    match spaces
        .iter()
        .map(|s| s.last_sync_at)
        .collect::<Option<Vec<_>>>()
        .and_then(|v| v.into_iter().min())
    {
        Some(at) => (
            t_args("sync-status-synced", &[("ago", Arg::str(ago(at, now_ms)))]),
            Tone::Quiet,
        ),
        None => (t("sync-status-never"), Tone::Quiet),
    }
}

/// "just now", "2 min ago", "3 h ago", "4 days ago".
pub fn ago(at_ms: i64, now_ms: i64) -> String {
    let secs = (now_ms - at_ms).max(0) / 1000;
    let (key, n) = match secs {
        0..60 => return t("ago-now"),
        60..3600 => ("ago-min", secs / 60),
        3600..86_400 => ("ago-hours", secs / 3600),
        _ => ("ago-days", secs / 86_400),
    };
    t_args(key, &[("n", Arg::count(u64::try_from(n).unwrap_or(0)))])
}

/// Whether a device can be forgotten: not this one, and not seen for 30
/// days.
pub fn can_forget(d: &DeviceRow, me: Option<Uuid>, now_ms: i64) -> bool {
    Some(d.install) != me && now_ms - d.last_seen >= FORGET_AFTER_MS
}

/// Which text an "Edited elsewhere" entry held.
pub fn edit_label(key: &str) -> String {
    match Key::parse(key) {
        Some(Key::Model {
            field: ModelField::Notes,
            ..
        }) => t("sync-edited-notes"),
        Some(Key::Recipe {
            field: RecipeField::Notes,
            ..
        }) => t("sync-edited-recipe"),
        Some(Key::Attempt {
            field: AttemptField::Notes,
            ..
        }) => t("sync-edited-attempt"),
        _ => t("sync-edited-other"),
    }
}

/// The text of an entry.
pub fn edit_text(e: &EditEntry) -> String {
    e.value.as_str().map(str::to_owned).unwrap_or_default()
}

/// What the user can do about a sync error.
pub fn sync_error_text(e: &SyncError) -> String {
    match e {
        SyncError::WrongPassphrase => t("sync-error-passphrase"),
        SyncError::PassphraseNeeded => t("sync-error-passphrase-needed"),
        SyncError::Credentials => t("backup-error-credentials"),
        SyncError::Unreachable(_) => t("sync-error-unreachable"),
        SyncError::Full => t("backup-error-full"),
        SyncError::ReadOnly => t("sync-error-read-only"),
        SyncError::NewerFormat(_) => t("sync-error-newer"),
        SyncError::AlreadyLinked => t("sync-error-already-linked"),
        SyncError::DeviceRecent => t("sync-error-device-recent"),
        SyncError::Truncated | SyncError::Corrupt => t("sync-error-damaged"),
        SyncError::SpaceNotFound | SyncError::NotLinked | SyncError::Storage(_) => t("error-other"),
    }
}

fn err_text(e: &CoreError) -> String {
    match e {
        CoreError::Sync(s) => sync_error_text(s),
        other => crate::app::error_text(other),
    }
}

type Res<T> = Result<T, Arc<CoreError>>;

#[derive(Clone, Debug)]
pub enum Sx {
    Loaded(Res<Loaded>),
    /// The sidebar line: opens its popover.
    TogglePopover,
    ClosePopover,
    /// Opens Settings › Sync.
    Open,
    /// "Sync this library…": Settings › Sync with the form for `lib`.
    SyncLibrary(LibraryId),
    SetEnabled(bool),
    DeviceInput(String),
    SaveDevice,
    Expand(i64),
    SyncNow,
    Synced(Res<()>),
    StartAdd,
    CancelAdd,
    Place(Place),
    AddLibrary(LibraryId),
    Kind(Kind),
    Input(Field, String),
    Encrypted(bool),
    Ack(bool),
    Remember(bool),
    PickFolder,
    FolderPicked(Option<PathBuf>),
    PickKeyFile,
    KeyFilePicked(Option<PathBuf>),
    Save,
    Added(Option<LibraryId>, Res<SpaceInfo>),
    StartLink(i64, Option<LibraryId>),
    LinkLoaded(i64, Option<LibraryId>, Res<Vec<RemoteLibrary>>),
    LinkLibrary(LibraryId),
    LinkChoice(Option<Uuid>),
    ConfirmLink,
    CancelLink,
    Unlink(i64, LibraryId),
    StartUnlock(i64),
    UnlockInput(String),
    UnlockRemember(bool),
    ConfirmUnlock,
    CancelUnlock,
    Unlocked(Res<()>),
    ShowParked(i64),
    ParkedLoaded(i64, Res<Vec<ParkedModel>>),
    HideParked,
    AskRemove(i64),
    CancelRemove,
    ConfirmRemove,
    AskForget(i64, Uuid),
    CancelForget,
    ConfirmForget,
    /// An action finished: reload.
    Done(Res<()>),
    DismissError,
    ModelLoaded(ModelId, Res<(Vec<EditEntry>, SyncAttribution)>),
    RestoreEdit(Uuid),
    CopyEdit(String),
    DismissEdit(Uuid),
    EditDone(Res<()>),
}

fn sx(m: Sx) -> Message {
    Message::Sync(m)
}

impl App {
    /// Loads Settings › Sync and the sidebar line.
    pub(crate) fn load_sync(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                Ok(Loaded {
                    enabled: engine.sync_enabled().await?,
                    device: engine.device_name().await?,
                    me: engine.install_id(),
                    spaces: engine.sync_status().await?,
                    queued: engine.sync_queued().await?,
                })
            },
            |r: Result<_, CoreError>| sx(Sx::Loaded(r.map_err(Arc::new))),
        )
    }

    /// Starts unattended sync (once the engine is open).
    pub(crate) fn start_sync(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        // The scheduler spawns tasks, so it starts on the runtime.
        let start = Task::perform(async move { engine.start_sync_scheduler() }, |()| {
            sx(Sx::Done(Ok(())))
        });
        Task::batch([start, self.load_sync()])
    }

    /// Loads the open model's edit history and device notes.
    pub(crate) fn load_model_sync(&self, id: ModelId) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        if self.sync.spaces.is_empty() && self.sync.model.history.is_empty() {
            return Task::none();
        }
        Task::perform(
            async move {
                let history = engine.edit_history(id).await?;
                let who = engine.sync_attribution(id).await?;
                Ok((history, who))
            },
            move |r: Result<_, CoreError>| sx(Sx::ModelLoaded(id, r.map_err(Arc::new))),
        )
    }

    fn add_space(&mut self) -> Task<Message> {
        let Some(a) = self.sync.add.as_mut() else {
            return Task::none();
        };
        if !a.can_save() {
            return Task::none();
        }
        a.saving = true;
        a.error = None;
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        let lib = a.library;
        let pass = a.passphrase();
        match a.place {
            Place::Library => {
                let Some(l) = lib else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.add_library_space(l, &pass).await.map_err(Arc::new) },
                    move |r| sx(Sx::Added(None, r)),
                )
            }
            Place::Target => {
                let Some(target) = a.form.target() else {
                    return Task::none();
                };
                let mut secrets = a.form.secrets();
                secrets.passphrase = String::new();
                let remember = a.form.remember;
                Task::perform(
                    async move {
                        engine
                            .add_target_space(target, &secrets, &pass, remember)
                            .await
                            .map_err(Arc::new)
                    },
                    move |r| sx(Sx::Added(lib, r)),
                )
            }
        }
    }

    fn open_link(&self, space: i64, lib: Option<LibraryId>) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.space_libraries(space).await.map_err(Arc::new) },
            move |r| sx(Sx::LinkLoaded(space, lib, r)),
        )
    }

    fn sync_now_task(&mut self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        self.sync
            .running
            .extend(self.sync.spaces.iter().map(|s| s.id));
        Task::perform(
            async move { engine.sync_now().await.map(|_| ()).map_err(Arc::new) },
            |r| sx(Sx::Synced(r)),
        )
    }

    fn after<T: Send + 'static>(
        &self,
        fut: impl Future<Output = Result<T, CoreError>> + Send + 'static,
    ) -> Task<Message> {
        Task::perform(
            async move { fut.await.map(|_| ()).map_err(Arc::new) },
            |r| sx(Sx::Done(r)),
        )
    }

    pub fn on_sync(&mut self, msg: Sx) -> Task<Message> {
        let engine = self.engine.clone();
        match msg {
            Sx::Loaded(Ok(l)) => {
                self.sync.loaded = true;
                self.sync.enabled = l.enabled;
                if self.sync.device == self.sync.device_saved {
                    self.sync.device.clone_from(&l.device);
                }
                self.sync.device_saved = l.device;
                self.sync.me = Some(l.me);
                self.sync.spaces = l.spaces;
                self.sync.queued = l.queued;
                Task::none()
            }
            Sx::Loaded(Err(e)) => {
                tracing::warn!(error = %e, "sync status");
                Task::none()
            }
            Sx::TogglePopover => {
                self.sync.popover = !self.sync.popover;
                if self.sync.popover {
                    self.load_sync()
                } else {
                    Task::none()
                }
            }
            Sx::ClosePopover => {
                self.sync.popover = false;
                Task::none()
            }
            Sx::Open => {
                self.sync.popover = false;
                self.screen = Screen::Settings;
                self.settings_section = SettingsSection::Sync;
                self.load_sync()
            }
            Sx::SyncLibrary(lib) => {
                self.screen = Screen::Settings;
                self.settings_section = SettingsSection::Sync;
                self.sync.add = Some(AddSpace::new(&self.libraries, Some(lib)));
                self.load_sync()
            }
            Sx::SetEnabled(on) => {
                self.sync.enabled = on;
                let Some(engine) = engine else {
                    return Task::none();
                };
                self.after(async move { engine.set_sync_enabled(on).await })
            }
            Sx::DeviceInput(s) => {
                self.sync.device = s;
                Task::none()
            }
            Sx::SaveDevice => {
                let Some(engine) = engine else {
                    return Task::none();
                };
                let name = self.sync.device.clone();
                self.after(async move { engine.set_device_name(&name).await })
            }
            Sx::Expand(id) => {
                if !self.sync.expanded.remove(&id) {
                    self.sync.expanded.insert(id);
                }
                Task::none()
            }
            Sx::SyncNow => {
                self.sync.popover = false;
                self.sync.error = None;
                self.sync_now_task()
            }
            Sx::Synced(r) => {
                self.sync.running.clear();
                if let Err(e) = r {
                    self.sync.error = Some(err_text(&e));
                }
                self.load_sync()
            }
            Sx::StartAdd => {
                self.sync.add = Some(AddSpace::new(&self.libraries, None));
                Task::none()
            }
            Sx::CancelAdd => {
                self.sync.add = None;
                Task::none()
            }
            Sx::Place(place) => {
                if let Some(a) = &mut self.sync.add {
                    a.set_place(place);
                }
                Task::none()
            }
            Sx::AddLibrary(lib) => {
                if let Some(a) = &mut self.sync.add {
                    a.library = Some(lib);
                    a.joining = false;
                }
                Task::none()
            }
            Sx::Kind(k) => {
                if let Some(a) = &mut self.sync.add {
                    a.set_kind(k);
                }
                Task::none()
            }
            Sx::Input(f, v) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.set(f, v);
                }
                Task::none()
            }
            Sx::Encrypted(on) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.encrypted = on;
                    a.joining = false;
                }
                Task::none()
            }
            Sx::Ack(on) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.ack = on;
                }
                Task::none()
            }
            Sx::Remember(on) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.remember = on;
                }
                Task::none()
            }
            Sx::PickFolder => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("sync-pick-folder"))
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                |p| sx(Sx::FolderPicked(p)),
            ),
            Sx::PickKeyFile => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("backup-pick-key"))
                        .pick_file()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                |p| sx(Sx::KeyFilePicked(p)),
            ),
            Sx::FolderPicked(Some(path)) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.set(Field::Path, path.display().to_string());
                }
                Task::none()
            }
            Sx::KeyFilePicked(Some(path)) => {
                if let Some(a) = &mut self.sync.add {
                    a.form.set(Field::KeyFile, path.display().to_string());
                }
                Task::none()
            }
            Sx::FolderPicked(None) | Sx::KeyFilePicked(None) => Task::none(),
            Sx::Save => self.add_space(),
            Sx::Added(lib, Ok(info)) => {
                self.sync.add = None;
                self.sync.expanded.insert(info.id);
                let id = info.id;
                let is_target = info.libraries.is_empty();
                let mut tasks = vec![self.load_sync()];
                if is_target {
                    tasks.push(self.open_link(id, lib));
                }
                tasks.push(self.sync_now_task());
                Task::batch(tasks)
            }
            Sx::Added(_, Err(e)) => {
                if let Some(a) = &mut self.sync.add {
                    a.saving = false;
                    if matches!(*e, CoreError::Sync(SyncError::PassphraseNeeded)) {
                        // The space exists and is encrypted: ask for its
                        // passphrase only.
                        a.form.encrypted = true;
                        a.joining = true;
                    }
                    a.error = Some(err_text(&e));
                }
                Task::none()
            }
            Sx::StartLink(space, lib) => self.open_link(space, lib),
            Sx::LinkLoaded(space, lib, Ok(remotes)) => {
                self.sync.link = Some(LinkDialog::new(space, lib, remotes));
                Task::none()
            }
            Sx::LinkLoaded(_, _, Err(e)) => {
                self.sync.error = Some(err_text(&e));
                Task::none()
            }
            Sx::LinkLibrary(lib) => {
                if let Some(d) = &mut self.sync.link {
                    d.set_library(lib);
                }
                Task::none()
            }
            Sx::LinkChoice(c) => {
                if let Some(d) = &mut self.sync.link {
                    d.choice = c;
                }
                Task::none()
            }
            Sx::ConfirmLink => {
                let Some(d) = self.sync.link.as_mut() else {
                    return Task::none();
                };
                let (Some(lib), Some(engine)) = (d.library, engine) else {
                    return Task::none();
                };
                d.busy = true;
                let (space, choice) = (d.space, d.choice);
                Task::perform(
                    async move {
                        engine.link_library(space, lib, choice).await?;
                        engine.sync_now().await.map(|_| ())
                    },
                    |r: Result<(), CoreError>| sx(Sx::Done(r.map_err(Arc::new))),
                )
                .chain(Task::done(sx(Sx::CancelLink)))
            }
            Sx::CancelLink => {
                self.sync.link = None;
                Task::none()
            }
            Sx::Unlink(space, lib) => {
                let Some(engine) = engine else {
                    return Task::none();
                };
                self.after(async move { engine.unlink_library(space, lib).await })
            }
            Sx::StartUnlock(space) => {
                self.sync.unlock = Some(Unlock {
                    space,
                    remember: true,
                    ..Unlock::default()
                });
                Task::none()
            }
            Sx::UnlockInput(s) => {
                if let Some(u) = &mut self.sync.unlock {
                    u.passphrase = s;
                    u.error = None;
                }
                Task::none()
            }
            Sx::UnlockRemember(on) => {
                if let Some(u) = &mut self.sync.unlock {
                    u.remember = on;
                }
                Task::none()
            }
            Sx::ConfirmUnlock => {
                let Some(u) = self.sync.unlock.as_mut() else {
                    return Task::none();
                };
                let Some(engine) = engine else {
                    return Task::none();
                };
                if u.passphrase.is_empty() {
                    return Task::none();
                }
                u.busy = true;
                let (space, pass, remember) = (u.space, u.passphrase.clone(), u.remember);
                Task::perform(
                    async move {
                        engine
                            .unlock_space(space, &pass, remember)
                            .await
                            .map_err(Arc::new)
                    },
                    |r| sx(Sx::Unlocked(r)),
                )
            }
            Sx::CancelUnlock => {
                self.sync.unlock = None;
                Task::none()
            }
            Sx::Unlocked(Ok(())) => {
                self.sync.unlock = None;
                Task::batch([self.load_sync(), self.sync_now_task()])
            }
            Sx::Unlocked(Err(e)) => {
                if let Some(u) = &mut self.sync.unlock {
                    u.busy = false;
                    u.error = Some(err_text(&e));
                }
                Task::none()
            }
            Sx::ShowParked(space) => {
                let Some(engine) = engine else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.parked_models(space).await.map_err(Arc::new) },
                    move |r| sx(Sx::ParkedLoaded(space, r)),
                )
            }
            Sx::ParkedLoaded(space, Ok(list)) => {
                self.sync.parked = Some((space, list));
                Task::none()
            }
            Sx::ParkedLoaded(_, Err(e)) | Sx::Done(Err(e)) | Sx::EditDone(Err(e)) => {
                self.sync.error = Some(err_text(&e));
                self.load_sync()
            }
            Sx::HideParked => {
                self.sync.parked = None;
                Task::none()
            }
            Sx::AskRemove(id) => {
                self.sync.confirm_remove = Some(id);
                Task::none()
            }
            Sx::CancelRemove => {
                self.sync.confirm_remove = None;
                Task::none()
            }
            Sx::ConfirmRemove => {
                let (Some(id), Some(engine)) = (self.sync.confirm_remove.take(), engine) else {
                    return Task::none();
                };
                self.sync.expanded.remove(&id);
                self.after(async move { engine.remove_space(id).await })
            }
            Sx::AskForget(space, install) => {
                self.sync.confirm_forget = Some((space, install));
                Task::none()
            }
            Sx::CancelForget => {
                self.sync.confirm_forget = None;
                Task::none()
            }
            Sx::ConfirmForget => {
                let (Some((space, install)), Some(engine)) =
                    (self.sync.confirm_forget.take(), engine)
                else {
                    return Task::none();
                };
                self.after(async move { engine.forget_device(space, install).await })
            }
            Sx::Done(Ok(())) => self.load_sync(),
            Sx::DismissError => {
                self.sync.error = None;
                Task::none()
            }
            Sx::ModelLoaded(id, r) => {
                if self.open_model() != Some(id) {
                    return Task::none();
                }
                match r {
                    Ok((history, attribution)) => {
                        self.sync.model = ModelSync {
                            id: Some(id),
                            history,
                            attribution,
                        };
                    }
                    Err(e) => tracing::warn!(error = %e, "model sync"),
                }
                Task::none()
            }
            Sx::RestoreEdit(entry) => {
                let Some(engine) = engine else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.restore_edit(entry).await.map_err(Arc::new) },
                    |r| sx(Sx::EditDone(r)),
                )
            }
            Sx::DismissEdit(entry) => {
                self.sync.model.history.retain(|e| e.id != entry);
                let Some(engine) = engine else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.dismiss_edit(entry).await.map_err(Arc::new) },
                    |r| sx(Sx::EditDone(r)),
                )
            }
            Sx::CopyEdit(text) => Task::batch([
                iced::clipboard::write(text),
                self.show_info(t("sync-edited-copied")),
            ]),
            Sx::EditDone(Ok(())) => match self.open_model() {
                Some(id) => Task::batch([self.load_model_sync(id), self.load_data(id)]),
                None => Task::none(),
            },
        }
    }

    /// Sync events from the engine.
    pub(crate) fn on_sync_event(&mut self, event: printdex_core::EngineEvent) -> Task<Message> {
        use printdex_core::EngineEvent as E;
        match event {
            E::SyncStarted { space } => {
                self.sync.running.insert(space);
                Task::none()
            }
            E::SyncFinished { space, .. } | E::SyncFailed { space, .. } => {
                self.sync.running.remove(&space);
                self.load_sync()
            }
            E::SyncParked { .. } => Task::none(),
            _ => Task::none(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use printdex_core::db::SpaceKind;

    fn lib(n: u128) -> Library {
        Library {
            id: LibraryId(Uuid::from_u128(n)),
            path: PathBuf::from(format!("/lib/{n}")),
            display_name: format!("Lib {n}"),
            kind: printdex_core::LibraryKind::Local,
            change_mode: printdex_core::ChangeMode::Auto,
            sidecar_enabled: false,
            last_full_scan_at: None,
        }
    }

    fn space(id: i64) -> SpaceInfo {
        SpaceInfo {
            id,
            kind: SpaceKind::Target,
            name: "NAS".into(),
            location: "/mnt/nas".into(),
            encrypted: false,
            libraries: vec![],
            devices: vec![],
            last_sync_at: Some(1_000_000),
            last_error: None,
            parked: 0,
            locked: false,
        }
    }

    #[test]
    fn add_space_flow_defaults() {
        let libs = [lib(1), lib(2)];
        let mut a = AddSpace::new(&libs, None);
        assert_eq!(a.place, Place::Library);
        assert_eq!(a.library, Some(libs[0].id));
        assert!(!a.form.encrypted);
        assert!(a.can_save());
        a.set_place(Place::Target);
        assert!(a.form.encrypted);
        // A folder target is encrypted too (it may be a cloud folder).
        a.set_kind(Kind::Folder);
        assert!(a.form.encrypted);
        a.form.set(Field::Path, "/mnt/share/sync".into());
        assert!(!a.can_save(), "needs a passphrase");
        a.form.passphrase = "secret".into();
        a.form.passphrase2 = "secret".into();
        a.form.ack = true;
        assert!(a.can_save());
        a.set_place(Place::Library);
        assert!(!a.form.encrypted);
        // From "Sync this library…".
        let b = AddSpace::new(&libs, Some(libs[1].id));
        assert_eq!(b.library, Some(libs[1].id));
    }

    #[test]
    fn joining_encrypted_space_asks_passphrase_once() {
        let mut a = AddSpace::new(&[lib(1)], None);
        a.form.encrypted = true;
        a.joining = true;
        a.form.passphrase = "secret".into();
        assert!(a.can_save());
    }

    #[test]
    fn link_dialog_preselects_suggestion() {
        let l1 = LibraryId(Uuid::from_u128(1));
        let l2 = LibraryId(Uuid::from_u128(2));
        let remote = |id: u128, suggested, linked| RemoteLibrary {
            id: Uuid::from_u128(id),
            name: format!("R{id}"),
            created_by: "Laptop".into(),
            linked,
            suggested,
        };
        let remotes = vec![
            remote(10, Some(l2), None),
            remote(11, Some(l1), None),
            remote(12, Some(l1), Some(l2)),
        ];
        let mut d = LinkDialog::new(4, Some(l1), remotes);
        assert_eq!(d.choice, Some(Uuid::from_u128(11)));
        assert_eq!(d.open_remotes().count(), 2);
        d.set_library(l2);
        assert_eq!(d.choice, Some(Uuid::from_u128(10)));
        // Nothing matches: a new library in the space.
        let d = LinkDialog::new(4, Some(LibraryId(Uuid::from_u128(3))), vec![]);
        assert_eq!(d.choice, None);
    }

    #[test]
    fn forget_disabled_recent_device() {
        let now = 100 * 24 * 3_600_000;
        let me = Uuid::from_u128(1);
        let d = |install, days_ago: i64| DeviceRow {
            install: Uuid::from_u128(install),
            name: "Laptop".into(),
            app_version: "0.1.0".into(),
            last_seen: now - days_ago * 24 * 3_600_000,
        };
        assert!(!can_forget(&d(2, 3), Some(me), now));
        assert!(can_forget(&d(2, 31), Some(me), now));
        assert!(!can_forget(&d(1, 90), Some(me), now));
    }

    #[test]
    fn status_text_cases() {
        let now = 1_000_000 + 2 * 60_000;
        assert_eq!(sync_status_text(true, &[], now).0, t("sync-status-off"));
        assert_eq!(
            sync_status_text(false, &[space(1)], now).0,
            t("sync-status-off")
        );
        let (s, tone) = sync_status_text(true, &[space(1)], now);
        assert_eq!(s, "Synced 2 min ago");
        assert_eq!(tone, Tone::Quiet);
        let mut down = space(2);
        down.last_error = Some("the sync space cannot be reached".into());
        let (s, tone) = sync_status_text(true, &[space(1), down], now);
        assert_eq!(s, "Waiting for NAS");
        assert_eq!(tone, Tone::Attention);
        let mut locked = space(3);
        locked.locked = true;
        assert_eq!(
            sync_status_text(true, &[space(1), locked], now).0,
            "Passphrase needed"
        );
        let mut never = space(4);
        never.last_sync_at = None;
        assert_eq!(
            sync_status_text(true, &[never], now).0,
            t("sync-status-never")
        );
    }

    #[test]
    fn ago_steps() {
        assert_eq!(ago(0, 30_000), "just now");
        assert_eq!(ago(0, 60_000), "1 min ago");
        assert_eq!(ago(0, 3 * 3_600_000), "3 h ago");
        assert_eq!(ago(0, 4 * 86_400_000), "4 days ago");
        assert_eq!(ago(0, 86_400_000), "1 day ago");
    }

    #[test]
    fn edit_labels() {
        let lib = Uuid::from_u128(1);
        let model = Uuid::from_u128(2);
        let k = Key::Model {
            lib,
            model,
            field: ModelField::Notes,
        };
        assert_eq!(edit_label(&k.text()), t("sync-edited-notes"));
        let k = Key::Attempt {
            lib,
            model,
            attempt: Uuid::from_u128(3),
            field: AttemptField::Notes,
        };
        assert_eq!(edit_label(&k.text()), t("sync-edited-attempt"));
    }
}
