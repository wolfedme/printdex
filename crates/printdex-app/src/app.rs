//! Application state and update logic (Elm style). Views live in `view`.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

use iced::futures::{Stream, stream};
use iced::keyboard::{self, key};
use iced::widget::scrollable::{AbsoluteOffset, RelativeOffset};
use iced::widget::{Id, operation};
use iced::{Event, Size, Subscription, Task, event, window};
use printdex_core::{
    CoreError, Engine, EngineEvent, Library, LibraryId, LibraryStatus, ModelDetail, ModelHit,
    ModelId, PartInfo, ScanPhase, ScanSummary, Scope, SortOrder, ThumbTarget,
};
use tokio_util::sync::CancellationToken;

use crate::filters::{self, Chip};
use crate::grid::GridMetrics;
use crate::i18n::{Arg, t, t_args};
use crate::motion::Motion;
use crate::nav::{self, Move};
use crate::org::{Org, OrgState};
use crate::prints::{Pr, PrintsUi};
use crate::search::{SEARCH_LIMIT, SearchState, Ticket};
use crate::settings::{Settings, ThemeMode, Translucency, ViewMode};
use crate::theme::{self, ColorTheme, Mode, Palette, Surface};
use crate::thumbs::Thumbs;
use crate::viewer::{self, Viewer};

/// Widget ids the update logic talks to.
pub fn search_id() -> Id {
    Id::new("search")
}

pub fn rename_id() -> Id {
    Id::new("rename")
}

pub fn grid_id() -> Id {
    Id::new("grid")
}

/// Toolbar height: 12px padding above and below a 32px control.
pub const TOOLBAR_HEIGHT: f32 = theme::S5 * 2.0 + theme::CONTROL_HEIGHT;
/// Space above the first and below the last grid row.
pub const GRID_PAD_Y: f32 = theme::S6;
/// Space left and right of the grid.
pub const GRID_PAD_X: f32 = theme::S7;
/// Initial window size (the DESIGN.md mock width).
pub const INITIAL_WINDOW: Size = Size::new(1280.0, 800.0);
/// Smallest window: the model header, the log form's side column and the
/// toolbar still fit, in their compact form.
pub const MIN_WINDOW: Size = Size::new(1024.0, 640.0);
/// Below this content width (window minus sidebar), toolbars, headers and
/// the status bar switch to their compact form: icon-only secondary
/// actions and fewer key hints.
pub const COMPACT_BELOW: f32 = 960.0;

/// Startup options from the command line and the settings file.
#[derive(Clone, Debug)]
pub struct Options {
    pub data_dir: PathBuf,
    /// Folder for caches (thumbnails); safe to delete.
    pub cache_dir: PathBuf,
    /// Where `settings.toml` lives; `None` keeps settings in memory only.
    pub config_dir: Option<PathBuf>,
    /// One-off light/dark override (`--mode`); never saved.
    pub mode: Option<Mode>,
    /// Settings as loaded at startup.
    pub settings: Settings,
    /// Whether the window was created transparent with an OS blur. Fixed
    /// for the window's lifetime.
    pub glass_window: bool,
}

impl Options {
    /// Options for tests and tools: no settings file, solid window.
    #[cfg(test)]
    pub fn in_memory(data_dir: PathBuf) -> Options {
        Options {
            cache_dir: data_dir.join("cache"),
            data_dir,
            config_dir: None,
            mode: None,
            settings: Settings::default(),
            glass_window: false,
        }
    }
}

/// The model view's content.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelView {
    Loading(ModelId),
    Loaded {
        detail: ModelDetail,
        part: usize,
    },
    /// Gone from the library (removed by a rescan, or never there).
    Missing(ModelId),
}

impl ModelView {
    pub fn id(&self) -> ModelId {
        match self {
            ModelView::Loading(id) | ModelView::Missing(id) => *id,
            ModelView::Loaded { detail, .. } => detail.id,
        }
    }
}

/// The section shown on the Settings screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsSection {
    #[default]
    Libraries,
    Appearance,
    Apps,
    Storage,
    Backup,
    About,
}

/// Which screen fills the main area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Screen {
    #[default]
    Library,
    Model(ModelId),
    Settings,
}

/// Startup state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Boot {
    Opening,
    Ready,
    Failed(PathBuf),
}

/// Progress of one library's running scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanState {
    pub phase: ScanPhase,
    pub done: u64,
    pub total: u64,
}

/// Grid scroll position and size, from the scrollable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridViewport {
    pub offset: f32,
    pub height: f32,
}

pub struct App {
    pub options: Options,
    pub engine: Option<Engine>,
    /// Multi-select, file action dialogs, undo toast, Open with.
    pub files: crate::files::FilesUi,
    pub updates: crate::updates::Updates,
    /// Settings › Backup and the sidebar status line.
    pub backup: crate::backup_ui::BackupUi,
    /// Restore wizard, library relinks and single-model restore.
    pub restore: crate::restore_ui::RestoreUi,
    /// Watches libraries for changes and offline shares while the app runs.
    pub monitor: Option<Arc<printdex_core::Monitor>>,
    pub boot: Boot,
    pub libraries: Vec<Library>,
    pub counts: HashMap<LibraryId, u64>,
    pub scans: HashMap<LibraryId, ScanState>,
    pub search: SearchState,
    pub selected: Option<ModelId>,
    pub hovered: Option<ModelId>,
    pub error: Option<String>,
    pub screen: Screen,
    pub model: Option<ModelView>,
    /// Active filter chips, in the order they were added.
    pub chips: Vec<Chip>,
    pub filter_menu: bool,
    pub settings_section: SettingsSection,
    /// The library whose "Remove" asks for confirmation.
    pub confirm_remove: Option<LibraryId>,
    /// Libraries whose folder can't be reached, with the time it was
    /// noticed ("14:02"). Their models stay browsable.
    pub offline: HashMap<LibraryId, String>,
    /// The library being renamed, with the edited name.
    pub renaming: Option<(LibraryId, String)>,
    pub settings: Settings,
    pub motion: Motion,
    /// The OS's light/dark preference, used when the mode is System.
    pub system_mode: Mode,
    pub window: Size,
    pub viewport: Option<GridViewport>,
    pub thumbs: Thumbs,
    /// The model view's 3D preview.
    pub viewer: Viewer,
    /// Thumbnail cache size on disk, when last measured.
    pub cache_usage: Option<u64>,
    pub clearing_cache: bool,
    /// Tags, collections, notes and source of the open model.
    pub org: OrgState,
    /// Recipe editor, print log and the "Log a print" form.
    pub prints: PrintsUi,
    cancel: CancellationToken,
    started: std::time::Instant,
}

/// Libraries and their present-model counts.
pub type LibrarySnapshot = (Vec<Library>, HashMap<LibraryId, u64>);

#[derive(Clone, Debug)]
pub enum Message {
    Booted(Result<Engine, Arc<CoreError>>),
    MonitorStarted(Arc<printdex_core::Monitor>),
    Files(crate::files::Fx),
    Updates(crate::updates::Ux),
    Backup(crate::backup_ui::Bx),
    Restore(crate::restore_ui::Rx),
    Libraries(Result<LibrarySnapshot, Arc<CoreError>>),
    Engine(EngineEvent),
    SearchChanged(String),
    Results(Ticket, Result<(Vec<ModelHit>, u64), Arc<CoreError>>),
    SelectScope(Scope),
    /// A sort picked inside a collection (not saved).
    QuerySort(SortOrder),
    Org(Org),
    Prints(Pr),
    /// Ctrl+Enter: saves whatever is being edited.
    SaveShortcut,
    PickLibrary,
    LibraryPicked(Option<PathBuf>),
    LibraryAdded(Result<Library, Arc<CoreError>>),
    ScanDone(LibraryId, Result<ScanSummary, Arc<CoreError>>),
    Rescan,
    Scrolled(GridViewport),
    WindowResized(Size),
    Select(ModelId),
    Hover(Option<ModelId>),
    Nav(Move),
    FocusSearch,
    Escape,
    SystemMode(iced::theme::Mode),
    DismissError,
    /// Open the selected model (Enter).
    OpenSelected,
    OpenModel(ModelId),
    OpenSettings,
    Back,
    ShowSection(SettingsSection),
    AskRemove(LibraryId),
    CancelRemove,
    ConfirmRemove(LibraryId),
    StartRename(LibraryId),
    RenameInput(String),
    CommitRename,
    CancelRename,
    /// Result of a remove or rename.
    LibraryChanged(Result<(), Arc<CoreError>>),
    RescanLibrary(LibraryId),
    /// A reachability check finished; `scan` starts a scan when online.
    Status(LibraryId, Result<LibraryStatus, Arc<CoreError>>, bool),
    ToggleFilterMenu,
    ToggleChip(Chip),
    ModelLoaded(ModelId, Result<Option<ModelDetail>, Arc<CoreError>>),
    SelectPart(usize),
    /// Open the selected part with the OS default app (O).
    OpenPart,
    /// Open a file with the OS default app.
    OpenFile(PathBuf),
    /// Show a file or folder in the OS file manager.
    ShowInFolder(PathBuf),
    /// Opens the folder a missing model was in (or its closest parent).
    ShowWhereItWas(PathBuf),
    Launched(Result<(), Arc<std::io::Error>>),
    SettingsChanged(Settings),
    /// A window frame while something animates.
    Frame(std::time::Instant),
    SettingsSaved(Result<(), Arc<std::io::Error>>),
    Viewer(viewer::Msg),
    /// Bytes the thumbnail cache uses.
    CacheUsage(u64),
    ClearCache,
    CacheCleared(Result<(), Arc<std::io::Error>>),
}

impl App {
    /// State before the engine is open.
    pub fn new(options: Options) -> App {
        let apps = options
            .config_dir
            .as_deref()
            .map(crate::apps::Apps::load)
            .unwrap_or_default();
        let settings = options.settings;
        let mut search = SearchState::default();
        search.query.sort = settings.sort;
        App {
            options,
            engine: None,
            monitor: None,
            updates: crate::updates::Updates::default(),
            backup: crate::backup_ui::BackupUi::default(),
            restore: crate::restore_ui::RestoreUi::default(),
            files: crate::files::FilesUi {
                apps,
                ..Default::default()
            },
            boot: Boot::Opening,
            libraries: Vec::new(),
            counts: HashMap::new(),
            scans: HashMap::new(),
            search,
            selected: None,
            hovered: None,
            error: None,
            screen: Screen::Library,
            model: None,
            motion: {
                let mut m = Motion::new(std::time::Instant::now());
                m.reduce = settings.reduce_motion;
                m
            },
            chips: Vec::new(),
            filter_menu: false,
            settings_section: SettingsSection::default(),
            confirm_remove: None,
            offline: HashMap::new(),
            renaming: None,
            settings,
            system_mode: Mode::default(),
            window: INITIAL_WINDOW,
            viewport: None,
            thumbs: Thumbs::default(),
            viewer: Viewer::default(),
            cache_usage: None,
            clearing_cache: false,
            org: OrgState::default(),
            prints: PrintsUi::default(),
            cancel: CancellationToken::new(),
            started: std::time::Instant::now(),
        }
    }

    /// Initial state and the tasks that open the engine and read the OS theme.
    pub fn boot(options: Options) -> (App, Task<Message>) {
        let dir = options.data_dir.clone();
        let cache = options.cache_dir.clone();
        let open = Task::perform(
            async move {
                Engine::open_with_cache(&dir, &cache)
                    .await
                    .map_err(Arc::new)
            },
            Message::Booted,
        );
        let mode = iced::system::theme().map(Message::SystemMode);
        let mut app = App::new(options);
        app.updates = crate::updates::Updates::start();
        let check = Task::done(Message::Updates(crate::updates::Ux::Tick));
        (app, Task::batch([open, mode, check]))
    }

    /// Light or dark right now: the `--mode` override, else the setting,
    /// else the OS.
    pub fn mode(&self) -> Mode {
        match (self.options.mode, self.settings.mode) {
            (Some(m), _) => m,
            (None, ThemeMode::Light) => Mode::Light,
            (None, ThemeMode::Dark) => Mode::Dark,
            (None, ThemeMode::System) => self.system_mode,
        }
    }

    /// Glass only in a transparent window and while translucency is on.
    pub fn surface(&self) -> Surface {
        if self.options.glass_window && self.settings.translucency == Translucency::Auto {
            Surface::Glass
        } else {
            Surface::Solid
        }
    }

    pub fn color_theme(&self) -> ColorTheme {
        self.settings.color_theme
    }

    /// The palette the settings ask for, without any cross-fade.
    pub fn target_palette(&self) -> Palette {
        Palette::new(self.color_theme(), self.mode(), self.surface())
    }

    /// The palette to draw now (mid cross-fade after a theme change).
    pub fn palette(&self) -> Palette {
        self.motion.palette(self.target_palette())
    }

    /// Runs `f`, cross-fading the colours if it changed the palette.
    fn fade_palette(&mut self, f: impl FnOnce(&mut App)) {
        let before = self.target_palette();
        let shown = self.palette();
        f(self);
        if self.target_palette() != before {
            self.motion
                .palette_changed(shown, std::time::Instant::now());
        }
    }

    fn set_selected(&mut self, id: Option<ModelId>) {
        self.motion
            .selection_changed(self.selected, id, std::time::Instant::now());
        self.selected = id;
    }

    pub fn title(&self) -> String {
        t("app-title")
    }

    pub fn theme(&self) -> iced::Theme {
        self.palette().iced_theme()
    }

    /// Grid layout for the current window width.
    pub fn metrics(&self) -> GridMetrics {
        GridMetrics::for_width(self.window.width - theme::SIDEBAR_WIDTH - 2.0 * GRID_PAD_X)
    }

    /// Whether the content area is narrow enough for compact chrome.
    pub fn compact(&self) -> bool {
        self.window.width - theme::SIDEBAR_WIDTH < COMPACT_BELOW
    }

    /// The grid viewport, estimated from the window until the scrollable
    /// reports its real size.
    pub fn grid_viewport(&self) -> GridViewport {
        self.viewport.unwrap_or(GridViewport {
            offset: 0.0,
            height: (self.window.height - TOOLBAR_HEIGHT - theme::STATUSBAR_HEIGHT).max(0.0),
        })
    }

    pub fn selected_index(&self) -> Option<usize> {
        let id = self.selected?;
        self.search.hits.iter().position(|h| h.id == id)
    }

    pub fn library(&self, id: LibraryId) -> Option<&Library> {
        self.libraries.iter().find(|l| l.id == id)
    }

    pub fn total_models(&self) -> u64 {
        self.counts.values().sum()
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        let task = self.handle(message);
        self.request_thumbnails();
        let viewer = self.sync_viewer();
        Task::batch([task, viewer])
    }

    /// Loads the selected part into the 3D preview, or closes the preview
    /// when no model is open.
    fn sync_viewer(&mut self) -> Task<Message> {
        let part = match self.screen {
            Screen::Model(_) => self.current_part().map(|p| p.id),
            _ => None,
        };
        match part {
            Some(id) => self
                .viewer
                .open(self.engine.as_ref(), id)
                .map(Message::Viewer),
            None => {
                if self.viewer.part.is_some() {
                    self.viewer.close();
                }
                Task::none()
            }
        }
    }

    /// Items on screen that show a thumbnail, most important first: the
    /// open model's selected part, its other parts, then grid or list
    /// items in reading order.
    pub fn visible_thumbnails(&self) -> Vec<ThumbTarget> {
        match self.screen {
            Screen::Model(_) => match &self.model {
                Some(ModelView::Loaded { detail, part }) => {
                    let mut out: Vec<ThumbTarget> = detail
                        .parts
                        .get(*part)
                        .map(|p| ThumbTarget::Part(p.id))
                        .into_iter()
                        .collect();
                    out.extend(detail.parts.iter().map(|p| ThumbTarget::Part(p.id)));
                    out
                }
                _ => Vec::new(),
            },
            Screen::Settings => Vec::new(),
            Screen::Library => {
                let hits = &self.search.hits;
                let vp = self.grid_viewport();
                let range = match self.settings.view {
                    ViewMode::Grid => {
                        let m = self.metrics();
                        let rows = m.visible_rows(vp.offset - GRID_PAD_Y, vp.height, hits.len());
                        rows.start * m.columns..(rows.end * m.columns).min(hits.len())
                    }
                    ViewMode::List => crate::list::visible_rows(vp.offset, vp.height, hits.len()),
                };
                hits[range]
                    .iter()
                    .map(|h| ThumbTarget::Model(h.id))
                    .collect()
            }
        }
    }

    /// Tells the engine what is on screen without a thumbnail, when that
    /// changed. Never blocks.
    fn request_thumbnails(&mut self) {
        let Some(engine) = &self.engine else {
            return;
        };
        if let Some(wanted) = self.thumbs.request_for(self.visible_thumbnails()) {
            engine.thumbnails().request_visible(wanted);
        }
    }

    fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Booted(Ok(engine)) => {
                // The monitor spawns tasks, so it starts on the runtime.
                let monitor = {
                    let engine = engine.clone();
                    Task::perform(
                        async move { Arc::new(engine.start_monitor()) },
                        Message::MonitorStarted,
                    )
                };
                self.engine = Some(engine);
                self.boot = Boot::Ready;
                let ticket = self.search.refresh();
                Task::batch([
                    monitor,
                    self.load_libraries(),
                    self.run_search(ticket),
                    self.apply_cache_limit(),
                    self.load_lists(),
                    self.start_backups(),
                    self.load_pending(),
                ])
            }
            Message::Files(f) => self.on_files(f),
            Message::Updates(u) => self.on_updates(u),
            Message::Backup(b) => self.on_backup(b),
            Message::Restore(r) => self.on_restore(r),
            Message::MonitorStarted(monitor) => {
                self.monitor = Some(monitor);
                Task::none()
            }
            Message::Booted(Err(e)) => {
                tracing::error!(error = %e, "could not open engine");
                self.boot = Boot::Failed(self.options.data_dir.clone());
                Task::none()
            }
            Message::Libraries(Ok((libraries, counts))) => {
                // Check reachability once for libraries seen for the first time.
                let checks: Vec<_> = libraries
                    .iter()
                    .filter(|l| self.library(l.id).is_none())
                    .map(|l| self.check_status(l.id, false))
                    .collect();
                self.libraries = libraries;
                self.counts = counts;
                self.offline
                    .retain(|id, _| self.libraries.iter().any(|l| l.id == *id));
                if let Scope::Library(scope) = self.search.query.scope
                    && self.library(scope).is_none()
                {
                    return Task::batch([self.set_scope(Scope::All), Task::batch(checks)]);
                }
                Task::batch(checks)
            }
            Message::Libraries(Err(e)) | Message::Results(_, Err(e)) => {
                self.show_error(&e);
                self.search.loading = false;
                Task::none()
            }
            Message::Engine(event) => self.on_engine_event(event),
            Message::QuerySort(sort) => match self.search.update(|q| q.sort = sort) {
                Some(ticket) => self.run_search(ticket),
                None => Task::none(),
            },
            Message::Org(msg) => self.handle_org(msg),
            Message::Prints(msg) => self.handle_prints(msg),
            Message::SaveShortcut => {
                if self.prints.form.is_some() {
                    self.update(Message::Prints(Pr::Save))
                } else if self.prints.note_for.is_some() {
                    self.update(Message::Prints(Pr::CommitNote))
                } else {
                    self.update(Message::Org(Org::SaveNotes))
                }
            }
            Message::SearchChanged(text) => match self.search.update(|q| q.text = text) {
                Some(ticket) => self.run_search(ticket),
                None => Task::none(),
            },
            Message::Results(ticket, Ok((hits, total))) => {
                if !self.search.loaded {
                    tracing::info!(
                        ms = self.started.elapsed().as_millis(),
                        models = total,
                        "library browsable"
                    );
                }
                if self.search.accept(&ticket, hits, total) && self.selected_index().is_none() {
                    self.selected = None;
                }
                Task::none()
            }
            Message::SelectScope(scope) => {
                self.screen = Screen::Library;
                self.model = None;
                self.org.cancel_edits();
                self.prints.reset();
                self.set_scope(scope)
            }
            Message::PickLibrary => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("sidebar-add-library"))
                        .pick_folder()
                        .await
                        .map(|h| h.path().to_path_buf())
                },
                Message::LibraryPicked,
            ),
            Message::LibraryPicked(None) => Task::none(),
            Message::LibraryPicked(Some(path)) => {
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.add_library(&path, None).await.map_err(Arc::new) },
                    Message::LibraryAdded,
                )
            }
            Message::LibraryAdded(Ok(lib)) => {
                self.error = None;
                let id = lib.id;
                if self.library(id).is_none() {
                    self.libraries.push(lib);
                }
                self.scans.insert(id, scan_starting());
                self.start_scan(id)
            }
            Message::LibraryAdded(Err(e)) => {
                self.show_error(&e);
                Task::none()
            }
            Message::ScanDone(id, result) => {
                self.scans.remove(&id);
                match result {
                    Ok(summary) => {
                        tracing::info!(?summary, "scan finished");
                        Task::none()
                    }
                    Err(e) if matches!(*e, CoreError::Cancelled) => Task::none(),
                    Err(e) if matches!(*e, CoreError::LibraryOffline(_)) => {
                        self.mark_offline(id);
                        Task::none()
                    }
                    Err(e) => {
                        self.show_error(&e);
                        Task::none()
                    }
                }
            }
            Message::Rescan => {
                let ids: Vec<LibraryId> = match self.search.query.scope {
                    Scope::Library(id) => vec![id],
                    _ => self.libraries.iter().map(|l| l.id).collect(),
                };
                let tasks: Vec<_> = ids
                    .into_iter()
                    .filter(|id| !self.scans.contains_key(id))
                    .map(|id| self.check_status(id, true))
                    .collect();
                Task::batch(tasks)
            }
            Message::Scrolled(viewport) => {
                self.viewport = Some(viewport);
                Task::none()
            }
            Message::WindowResized(size) => {
                self.window = size;
                Task::none()
            }
            Message::Select(id) => {
                self.motion.pressed(id, std::time::Instant::now());
                self.click_select(id);
                self.set_selected(Some(id));
                Task::none()
            }
            Message::Hover(id) => {
                self.motion
                    .hover_changed(self.hovered, id, std::time::Instant::now());
                self.hovered = id;
                Task::none()
            }
            Message::Nav(mv) if matches!(self.screen, Screen::Model(_)) => {
                self.step_part(mv);
                Task::none()
            }
            Message::Nav(_) if self.screen == Screen::Settings => Task::none(),
            Message::Nav(mv) => self.navigate(mv),
            Message::FocusSearch => Task::batch([
                operation::focus(search_id()),
                operation::select_all(search_id()),
            ]),
            Message::Escape
                if self.files.dialog.is_some()
                    || self.files.open_menu
                    || self.files.material_menu =>
            {
                self.update(Message::Files(crate::files::Fx::Cancel))
            }
            Message::Escape if !self.files.picked.is_empty() && self.screen == Screen::Library => {
                self.update(Message::Files(crate::files::Fx::ClearSelection))
            }
            Message::Escape if self.prints.open() => {
                if self.prints.form.is_some() {
                    self.prints.form = None;
                    self.prints.saving = false;
                } else {
                    self.prints.confirm_delete = None;
                    self.prints.note_for = None;
                }
                Task::none()
            }
            Message::Escape if self.org.editing() => self.update(Message::Org(Org::Cancel)),
            Message::Escape if self.org.notice.is_some() && self.screen == Screen::Library => {
                self.org.notice = None;
                Task::none()
            }
            Message::Escape if self.renaming.is_some() => self.update(Message::CancelRename),
            Message::Escape if self.confirm_remove.is_some() => self.update(Message::CancelRemove),
            Message::Escape if self.screen != Screen::Library => self.update(Message::Back),
            Message::Escape if self.filter_menu => {
                self.filter_menu = false;
                Task::none()
            }
            Message::Escape => {
                if self.search.query.text.is_empty() {
                    self.set_selected(None);
                    Task::none()
                } else {
                    self.update(Message::SearchChanged(String::new()))
                }
            }
            Message::SystemMode(mode) => {
                let mode = match mode {
                    iced::theme::Mode::Dark => Mode::Dark,
                    iced::theme::Mode::Light | iced::theme::Mode::None => Mode::Light,
                };
                self.fade_palette(|app| app.system_mode = mode);
                Task::none()
            }
            Message::Frame(now) => {
                self.motion.now = now;
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::ShowSection(section) => {
                self.settings_section = section;
                match section {
                    SettingsSection::Storage => self.measure_cache(),
                    SettingsSection::Backup => self.load_backup(),
                    _ => Task::none(),
                }
            }
            Message::CacheUsage(bytes) => {
                self.cache_usage = Some(bytes);
                Task::none()
            }
            Message::ClearCache => {
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                self.clearing_cache = true;
                Task::perform(
                    async move { engine.thumbnails().clear_cache().await.map_err(Arc::new) },
                    Message::CacheCleared,
                )
            }
            Message::CacheCleared(result) => {
                self.clearing_cache = false;
                if let Err(e) = result {
                    tracing::warn!(error = %e, "could not clear the thumbnail cache");
                    self.error = Some(t("error-cache-clear"));
                }
                // Thumbnails are rendered again as they come on screen.
                self.thumbs.clear();
                self.measure_cache()
            }
            Message::AskRemove(id) => {
                self.confirm_remove = Some(id);
                Task::none()
            }
            Message::CancelRemove => {
                self.confirm_remove = None;
                Task::none()
            }
            Message::ConfirmRemove(id) => {
                // Only the library whose confirmation is showing.
                if self.confirm_remove.take() != Some(id) {
                    return Task::none();
                }
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.remove_library(id).await.map_err(Arc::new) },
                    Message::LibraryChanged,
                )
            }
            Message::StartRename(id) => {
                let name = self
                    .library(id)
                    .map(|l| l.display_name.clone())
                    .unwrap_or_default();
                self.renaming = Some((id, name));
                Task::batch([
                    operation::focus(rename_id()),
                    operation::select_all(rename_id()),
                ])
            }
            Message::RenameInput(name) => {
                if let Some((_, current)) = &mut self.renaming {
                    *current = name;
                }
                Task::none()
            }
            Message::CancelRename => {
                self.renaming = None;
                Task::none()
            }
            Message::CommitRename => {
                let Some((id, name)) = self.renaming.take() else {
                    return Task::none();
                };
                if name.trim().is_empty()
                    || self.library(id).map(|l| l.display_name.as_str()) == Some(name.trim())
                {
                    return Task::none();
                }
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        engine
                            .rename_library(id, &name)
                            .await
                            .map(|_| ())
                            .map_err(Arc::new)
                    },
                    Message::LibraryChanged,
                )
            }
            Message::LibraryChanged(Ok(())) => Task::none(),
            Message::LibraryChanged(Err(e)) => {
                self.show_error(&e);
                Task::none()
            }
            Message::RescanLibrary(id) => {
                if self.scans.contains_key(&id) {
                    return Task::none();
                }
                self.check_status(id, true)
            }
            Message::Status(id, Ok(LibraryStatus::Offline), _) => {
                self.mark_offline(id);
                Task::none()
            }
            Message::Status(id, Ok(LibraryStatus::Online), scan) => {
                self.offline.remove(&id);
                if scan && let std::collections::hash_map::Entry::Vacant(e) = self.scans.entry(id) {
                    e.insert(scan_starting());
                    return self.start_scan(id);
                }
                Task::none()
            }
            Message::Status(_, Err(e), _) => {
                self.show_error(&e);
                Task::none()
            }
            Message::ToggleFilterMenu => {
                self.filter_menu = !self.filter_menu;
                Task::none()
            }
            Message::ToggleChip(chip) => {
                filters::toggle(&mut self.chips, chip);
                if self.chips.contains(&chip) {
                    self.motion.chip_added(chip, std::time::Instant::now());
                }
                let filter = filters::to_filter(&self.chips);
                match self.search.update(|q| q.filter = filter) {
                    Some(ticket) => self.run_search(ticket),
                    None => Task::none(),
                }
            }
            Message::OpenSelected if self.files.dialog.is_some() => {
                self.update(Message::Files(crate::files::Fx::Confirm))
            }
            // Enter opens only a single selection.
            Message::OpenSelected if self.files.picked.len() > 1 => Task::none(),
            Message::OpenSelected => match (self.screen, self.selected) {
                (Screen::Library, Some(id)) => self.update(Message::OpenModel(id)),
                _ => Task::none(),
            },
            Message::OpenModel(id) => {
                self.selected = Some(id);
                self.screen = Screen::Model(id);
                self.model = Some(ModelView::Loading(id));
                self.org.cancel_edits();
                self.prints.reset();
                if self.org.data.as_ref().is_some_and(|(m, _)| *m != id) {
                    self.org.data = None;
                    self.org.notes_md.clear();
                }
                if self.restore.model.model != Some(id) {
                    self.restore.model = crate::restore_ui::ModelRestore::default();
                }
                Task::batch([self.load_model(id), self.load_data(id)])
            }
            Message::ModelLoaded(id, result) => {
                // A load for a model that is no longer on screen is stale.
                if self.model.as_ref().map(ModelView::id) != Some(id) {
                    return Task::none();
                }
                let part = match &self.model {
                    Some(ModelView::Loaded { part, .. }) => *part,
                    _ => 0,
                };
                self.model = Some(match result {
                    Ok(Some(detail)) => {
                        let part = part.min(detail.parts.len().saturating_sub(1));
                        ModelView::Loaded { detail, part }
                    }
                    Ok(None) => ModelView::Missing(id),
                    Err(e) => {
                        self.show_error(&e);
                        ModelView::Missing(id)
                    }
                });
                Task::none()
            }
            Message::SelectPart(index) => {
                if let Some(ModelView::Loaded { detail, part }) = &mut self.model
                    && index < detail.parts.len()
                {
                    self.motion
                        .part_changed(*part, index, std::time::Instant::now());
                    *part = index;
                }
                Task::none()
            }
            Message::OpenPart if matches!(self.screen, Screen::Model(_)) => self.open_part(),
            Message::OpenPart => Task::none(),
            Message::OpenFile(path) => launch(move || opener::open(&path)),
            Message::ShowInFolder(path) => launch(move || opener::reveal(&path)),
            Message::ShowWhereItWas(path) => launch(move || opener::open(nearest_existing(&path))),
            Message::Launched(Ok(())) => Task::none(),
            Message::Launched(Err(e)) => {
                tracing::warn!(error = %e, "could not hand the file to the OS");
                self.error = Some(t("error-launch"));
                Task::none()
            }
            Message::OpenSettings => {
                self.screen = Screen::Settings;
                Task::none()
            }
            Message::Back => {
                self.screen = Screen::Library;
                self.model = None;
                self.org.cancel_edits();
                self.prints.reset();
                Task::none()
            }
            Message::SettingsChanged(settings) => {
                let mut old = self.settings;
                self.fade_palette(|app| old = std::mem::replace(&mut app.settings, settings));
                self.motion.reduce = settings.reduce_motion;
                if old.view != settings.view {
                    // The other layout starts at the top.
                    self.viewport = None;
                }
                let search = if old.sort == settings.sort {
                    Task::none()
                } else {
                    match self.search.update(|q| q.sort = settings.sort) {
                        Some(ticket) => self.run_search(ticket),
                        None => Task::none(),
                    }
                };
                let cache = if old.cache_mb == settings.cache_mb {
                    Task::none()
                } else {
                    self.apply_cache_limit()
                };
                Task::batch([search, cache, self.save_settings()])
            }
            Message::Viewer(msg) => self
                .viewer
                .update(self.engine.as_ref(), msg)
                .map(Message::Viewer),
            Message::SettingsSaved(Ok(())) => Task::none(),
            Message::SettingsSaved(Err(e)) => {
                tracing::warn!(error = %e, "could not save settings");
                self.error = Some(t("error-settings-save"));
                Task::none()
            }
        }
    }

    fn mark_offline(&mut self, id: LibraryId) {
        self.offline
            .entry(id)
            .or_insert_with(|| jiff::ZonedDateTime::now().strftime("%H:%M").to_string());
    }

    fn check_status(&self, id: LibraryId, scan: bool) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.library_status(id).await.map_err(Arc::new) },
            move |r| Message::Status(id, r, scan),
        )
    }

    /// The part selected in the model view.
    /// Absolute path of the open model's current part.
    pub fn current_part_path(&self) -> Option<PathBuf> {
        let rel = self.current_part()?.rel_path.clone();
        self.model_path(&rel)
    }

    pub fn current_part(&self) -> Option<&PartInfo> {
        match &self.model {
            Some(ModelView::Loaded { detail, part }) => detail.parts.get(*part),
            _ => None,
        }
    }

    /// Absolute path of a file of the open model (`rel` is library-relative).
    pub fn model_path(&self, rel: &str) -> Option<PathBuf> {
        let Some(ModelView::Loaded { detail, .. }) = &self.model else {
            return None;
        };
        let lib = self.library(detail.library_id)?;
        Some(rel.split('/').fold(lib.path.clone(), |p, seg| p.join(seg)))
    }

    fn step_part(&mut self, mv: Move) {
        if let Some(ModelView::Loaded { detail, part }) = &mut self.model {
            let n = detail.parts.len();
            if n == 0 {
                return;
            }
            let next = match mv {
                Move::Left => part.saturating_sub(1),
                Move::Right => (*part + 1).min(n - 1),
                Move::Home => 0,
                Move::End => n - 1,
                Move::Up | Move::Down => *part,
            };
            self.motion
                .part_changed(*part, next, std::time::Instant::now());
            *part = next;
        }
    }

    pub(crate) fn load_model(&self, id: ModelId) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.model(id).await.map_err(Arc::new) },
            move |r| Message::ModelLoaded(id, r),
        )
    }

    /// Hands the cache limit to the engine, which trims the cache to it,
    /// then measures the cache.
    fn apply_cache_limit(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        let limit = self.settings.cache_bytes();
        Task::perform(
            async move {
                let thumbs = engine.thumbnails();
                thumbs.set_cache_limit(limit).await;
                thumbs.cache_usage().await
            },
            Message::CacheUsage,
        )
    }

    fn measure_cache(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.thumbnails().cache_usage().await },
            Message::CacheUsage,
        )
    }

    /// Writes the settings file off the UI thread.
    pub(crate) fn save_settings(&self) -> Task<Message> {
        let Some(dir) = self.options.config_dir.clone() else {
            return Task::none();
        };
        let settings = self.settings;
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || settings.save(&dir))
                    .await
                    .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                    .map_err(Arc::new)
            },
            Message::SettingsSaved,
        )
    }

    fn on_engine_event(&mut self, event: EngineEvent) -> Task<Message> {
        match event {
            EngineEvent::LibraryAdded(_) => Task::none(),
            EngineEvent::LibraryRenamed(lib) => {
                if let Some(l) = self.libraries.iter_mut().find(|l| l.id == lib.id) {
                    *l = lib;
                }
                let ticket = self.search.refresh();
                Task::batch([self.load_libraries(), self.run_search(ticket)])
            }
            EngineEvent::LibraryRemoved(id) => {
                self.libraries.retain(|l| l.id != id);
                self.counts.remove(&id);
                let ticket = self.search.refresh();
                Task::batch([self.load_libraries(), self.run_search(ticket)])
            }
            EngineEvent::ScanStarted { library } => {
                self.scans.insert(library, scan_starting());
                Task::none()
            }
            EngineEvent::ScanProgress {
                library,
                phase,
                done,
                total,
            } => {
                self.scans.insert(library, ScanState { phase, done, total });
                Task::none()
            }
            EngineEvent::ScanFinished { library, summary } => {
                self.scans.remove(&library);
                if summary.fingerprinted > 0 {
                    // New or changed files: their thumbnails are new.
                    self.thumbs.clear();
                }
                let ticket = self.search.refresh();
                let model = match &self.model {
                    Some(m) => Task::batch([self.load_model(m.id()), self.load_data(m.id())]),
                    None => Task::none(),
                };
                Task::batch([
                    self.load_libraries(),
                    self.run_search(ticket),
                    model,
                    self.load_lists(),
                ])
            }
            EngineEvent::ModelDataChanged(id) => {
                // Tags and recipes change what cards show and what matches.
                let ticket = self.search.refresh();
                let open = self.model.as_ref().map(ModelView::id) == Some(id);
                if open {
                    // The preview material may have changed.
                    let mut targets = vec![ThumbTarget::Model(id)];
                    if let Some(ModelView::Loaded { detail, .. }) = &self.model {
                        targets.extend(detail.parts.iter().map(|p| ThumbTarget::Part(p.id)));
                    }
                    self.thumbs.refresh(targets);
                } else {
                    self.thumbs.refresh([ThumbTarget::Model(id)]);
                }
                let data = if open {
                    self.load_data(id)
                } else {
                    Task::none()
                };
                // Smart filter counts follow the print log.
                Task::batch([self.run_search(ticket), data, self.load_lists()])
            }
            EngineEvent::FileOpDone(op) => {
                let toast = self.file_op_done(op);
                // Names and paths changed: reload what shows them.
                let ticket = self.search.refresh();
                let model = match &self.model {
                    Some(m) => Task::batch([self.load_model(m.id()), self.load_data(m.id())]),
                    None => Task::none(),
                };
                Task::batch([
                    toast,
                    self.load_libraries(),
                    self.run_search(ticket),
                    model,
                    self.load_lists(),
                ])
            }
            EngineEvent::LibraryOffline(id) => {
                self.mark_offline(id);
                Task::none()
            }
            EngineEvent::LibraryOnline(id) => {
                self.offline.remove(&id);
                Task::none()
            }
            EngineEvent::ModelForgotten(id) => {
                if self.model.as_ref().map(ModelView::id) == Some(id) {
                    self.model = None;
                    self.org.cancel_edits();
                    self.prints.reset();
                }
                if self.selected == Some(id) {
                    self.selected = None;
                }
                let ticket = self.search.refresh();
                Task::batch([
                    self.load_libraries(),
                    self.run_search(ticket),
                    self.load_lists(),
                ])
            }
            EngineEvent::OrganizeChanged => {
                let ticket = self.search.refresh();
                Task::batch([self.load_lists(), self.run_search(ticket)])
            }
            EngineEvent::SidecarForeign { library } => {
                let name = self
                    .library(library)
                    .map(|l| l.display_name.clone())
                    .unwrap_or_default();
                self.org.notice = Some(t_args(
                    "notice-sidecar-foreign",
                    &[("library", Arg::str(name))],
                ));
                Task::none()
            }
            EngineEvent::LibraryChanged(lib) => {
                if let Some(l) = self.libraries.iter_mut().find(|l| l.id == lib.id) {
                    *l = lib;
                }
                Task::none()
            }
            EngineEvent::ThumbnailReady {
                target,
                path,
                kind,
                print_in,
            } => {
                if let Some(top) = print_in {
                    self.motion.print_in(target, top, std::time::Instant::now());
                }
                self.thumbs.ready(target, path, kind);
                Task::none()
            }
            EngineEvent::ThumbnailFailed { target, reason } => {
                self.thumbs.failed(target, reason);
                Task::none()
            }
            EngineEvent::ScanFailed { library, error } => {
                self.scans.remove(&library);
                match *error {
                    CoreError::Cancelled => {}
                    CoreError::LibraryOffline(_) => self.mark_offline(library),
                    _ => self.show_error(&error),
                }
                Task::none()
            }
            event @ (EngineEvent::BackupStarted { .. }
            | EngineEvent::BackupProgress { .. }
            | EngineEvent::BackupFinished { .. }
            | EngineEvent::BackupFailed { .. }
            | EngineEvent::BackupNeedsAttention(_)) => self.on_backup_event(event),
        }
    }

    pub(crate) fn set_scope(&mut self, scope: Scope) -> Task<Message> {
        self.org.collection_edit = None;
        self.viewport = self.viewport.map(|v| GridViewport { offset: 0.0, ..v });
        let scroll = operation::snap_to(grid_id(), RelativeOffset::START);
        // A collection keeps its own order; elsewhere the saved sort applies.
        let sort = if matches!(scope, Scope::Collection(_)) {
            SortOrder::Collection
        } else {
            self.settings.sort
        };
        match self.search.update(|q| {
            q.scope = scope;
            q.sort = sort;
        }) {
            Some(ticket) => Task::batch([self.run_search(ticket), scroll]),
            None => Task::none(),
        }
    }

    fn navigate(&mut self, mv: Move) -> Task<Message> {
        let metrics = self.metrics();
        let len = self.search.hits.len();
        let columns = match self.settings.view {
            ViewMode::Grid => metrics.columns,
            ViewMode::List => 1,
        };
        let Some(index) = nav::step(self.selected_index(), mv, columns, len) else {
            return Task::none();
        };
        self.set_selected(Some(self.search.hits[index].id));
        if self.settings.view == ViewMode::List {
            let vp = self.grid_viewport();
            return match crate::list::scroll_target(index, vp.offset, vp.height) {
                Some(y) => operation::scroll_to(grid_id(), AbsoluteOffset { x: 0.0, y }),
                None => Task::none(),
            };
        }
        let row = index / metrics.columns.max(1);
        let vp = self.grid_viewport();
        // Rows start below the grid's top padding: row coordinates are
        // content coordinates minus GRID_PAD_Y.
        match metrics.scroll_target(row, vp.offset - GRID_PAD_Y, vp.height) {
            Some(target) => {
                let y = if row == 0 {
                    0.0
                } else {
                    (target + GRID_PAD_Y).max(0.0)
                };
                operation::scroll_to(grid_id(), AbsoluteOffset { x: 0.0, y })
            }
            None => Task::none(),
        }
    }

    pub(crate) fn show_error(&mut self, e: &CoreError) {
        tracing::warn!(error = %e, "engine error");
        self.error = Some(error_text(e));
    }

    pub(crate) fn load_libraries(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let libraries = engine.libraries().await?;
                let counts = engine.library_counts().await?;
                Ok((libraries, counts))
            },
            |r: Result<_, CoreError>| Message::Libraries(r.map_err(Arc::new)),
        )
    }

    fn run_search(&self, ticket: Ticket) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                let result = async {
                    let q = &ticket.query;
                    let hits = engine
                        .search_in(&q.scope, &q.text, &q.filter, q.sort, SEARCH_LIMIT)
                        .await?;
                    let total = if hits.len() < SEARCH_LIMIT as usize {
                        hits.len() as u64
                    } else {
                        engine.count(&q.scope, &q.text, &q.filter).await?
                    };
                    Ok::<_, CoreError>((hits, total))
                }
                .await;
                (ticket, result.map_err(Arc::new))
            },
            |(ticket, result)| Message::Results(ticket, result),
        )
    }

    fn start_scan(&self, id: LibraryId) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        let cancel = self.cancel.child_token();
        Task::perform(
            async move { engine.rescan(id, cancel).await.map_err(Arc::new) },
            move |r| Message::ScanDone(id, r),
        )
    }

    pub fn subscription(&self) -> Subscription<Message> {
        let mut subs = vec![
            event::listen_with(on_event),
            iced::system::theme_changes().map(Message::SystemMode),
        ];
        if self.settings.update_checks == Some(true) && self.updates.available() {
            subs.push(
                iced::time::every(std::time::Duration::from_secs(3600))
                    .map(|_| Message::Updates(crate::updates::Ux::Tick)),
            );
        }
        if self.motion.animating(std::time::Instant::now()) {
            subs.push(window::frames().map(Message::Frame));
        }
        if let Some(engine) = &self.engine {
            subs.push(Subscription::run_with(
                EngineSub(engine.clone()),
                engine_events,
            ));
        }
        Subscription::batch(subs)
    }
}

/// `path`, or its closest ancestor that still exists (a missing model's
/// folder is gone; its parent usually is not).
fn nearest_existing(path: &std::path::Path) -> PathBuf {
    let mut p = path.to_path_buf();
    while !p.exists() && p.pop() {}
    p
}

/// Runs an OS hand-off (open, reveal) off the UI thread.
pub(crate) fn launch(
    f: impl FnOnce() -> Result<(), opener::OpenError> + Send + 'static,
) -> Task<Message> {
    Task::perform(
        async move {
            tokio::task::spawn_blocking(move || f().map_err(std::io::Error::other))
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                .map_err(Arc::new)
        },
        Message::Launched,
    )
}

fn scan_starting() -> ScanState {
    ScanState {
        phase: ScanPhase::Walking,
        done: 0,
        total: 0,
    }
}

/// A translated sentence for an engine error.
pub fn error_text(e: &CoreError) -> String {
    match e {
        CoreError::LibraryOverlaps { existing } => t_args(
            "error-library-overlaps",
            &[("path", Arg::str(existing.display().to_string()))],
        ),
        CoreError::LibraryPathInvalid(_) => t("error-library-invalid"),
        CoreError::LibraryOffline(path) => t_args(
            "error-library-offline",
            &[("path", Arg::str(path.display().to_string()))],
        ),
        CoreError::AlreadyScanning(_) => t("error-already-scanning"),
        CoreError::TagInvalid => t("error-tag-invalid"),
        CoreError::SourceInvalid => t("error-source-invalid"),
        CoreError::CollectionExists => t("error-collection-exists"),
        CoreError::CollectionNameInvalid => t("error-collection-name"),
        CoreError::TextTooLong => t("error-text-too-long"),
        CoreError::Backup(b) => crate::backup_ui::backup_error_text(b),
        _ => t("error-other"),
    }
}

/// Maps window and keyboard events to messages.
fn on_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        Event::Window(window::Event::FileDropped(path)) => {
            Some(Message::Prints(Pr::FileDropped(path)))
        }
        Event::Keyboard(keyboard::Event::ModifiersChanged(m)) => {
            Some(Message::Files(crate::files::Fx::Modifiers(m)))
        }
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            let free = status == event::Status::Ignored;
            use crate::files::Fx;
            match key.as_ref() {
                keyboard::Key::Named(key::Named::Escape) => Some(Message::Escape),
                keyboard::Key::Named(key::Named::F2) if free => Some(Message::Files(Fx::AskRename)),
                keyboard::Key::Named(key::Named::Delete) if free => {
                    Some(Message::Files(Fx::AskTrash))
                }
                keyboard::Key::Character("a") if free && modifiers.command() => {
                    Some(Message::Files(Fx::SelectAll))
                }
                keyboard::Key::Character("z") if free && modifiers.command() => {
                    Some(Message::Files(Fx::UndoLast))
                }

                keyboard::Key::Character(",") if modifiers.command() => Some(Message::OpenSettings),
                keyboard::Key::Named(key::Named::Enter) if modifiers.command() => {
                    Some(Message::SaveShortcut)
                }
                keyboard::Key::Named(key::Named::Enter) if free => Some(Message::OpenSelected),
                keyboard::Key::Character("o") if free && !modifiers.command() => {
                    Some(Message::OpenPart)
                }
                keyboard::Key::Character("f") if modifiers.command() => Some(Message::FocusSearch),
                keyboard::Key::Character("t") if free && !modifiers.command() => {
                    Some(Message::Org(Org::StartTag))
                }
                keyboard::Key::Character("n") if free && !modifiers.command() => {
                    Some(Message::Org(Org::StartNotes))
                }
                keyboard::Key::Character("l") if free && !modifiers.command() => {
                    Some(Message::Prints(Pr::OpenLog))
                }
                keyboard::Key::Character("/") if free => Some(Message::FocusSearch),
                keyboard::Key::Named(named) if free => match named {
                    key::Named::ArrowLeft => Some(Message::Nav(Move::Left)),
                    key::Named::ArrowRight => Some(Message::Nav(Move::Right)),
                    key::Named::ArrowUp => Some(Message::Nav(Move::Up)),
                    key::Named::ArrowDown => Some(Message::Nav(Move::Down)),
                    key::Named::Home => Some(Message::Nav(Move::Home)),
                    key::Named::End => Some(Message::Nav(Move::End)),
                    _ => None,
                },
                _ => None,
            }
        }
        _ => None,
    }
}

/// Subscription identity for the engine event stream (one engine per app).
struct EngineSub(Engine);

impl Hash for EngineSub {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "printdex-engine-events".hash(state);
    }
}

fn engine_events(sub: &EngineSub) -> impl Stream<Item = Message> + use<> {
    let rx = sub.0.subscribe();
    stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(event) => return Some((Message::Engine(event), rx)),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!(n, "UI lagged behind engine events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use printdex_core::ModelKind;

    fn app() -> App {
        App::new(Options::in_memory(PathBuf::from("/nonexistent")))
    }

    fn hit(name: &str) -> ModelHit {
        ModelHit {
            id: ModelId::new(),
            library_id: LibraryId::new(),
            name: name.into(),
            rel_path: name.into(),
            kind: ModelKind::Folder,
            number: None,
            part_count: 1,
            missing: false,
            created_at: 0,
            material: None,
            recipe: None,
            last_print: None,
        }
    }

    /// Delivers `hits` as the results of a fresh run of the current query.
    fn deliver(app: &mut App, hits: Vec<ModelHit>) {
        let ticket = app.search.refresh();
        let total = hits.len() as u64;
        let _ = app.update(Message::Results(ticket, Ok((hits, total))));
    }

    fn four(a: &mut App) -> Vec<ModelId> {
        let hits: Vec<ModelHit> = ["a", "b", "c", "d"].into_iter().map(hit).collect();
        let ids = hits.iter().map(|h| h.id).collect();
        deliver(a, hits);
        ids
    }

    fn with_mods(a: &mut App, m: keyboard::Modifiers) {
        let _ = a.update(Message::Files(crate::files::Fx::Modifiers(m)));
    }

    #[test]
    fn ctrl_click_toggles_selection() {
        let mut a = app();
        let ids = four(&mut a);
        let _ = a.update(Message::Select(ids[0]));
        assert!(a.files.picked.is_empty(), "a plain click picks one");
        with_mods(&mut a, keyboard::Modifiers::COMMAND);
        let _ = a.update(Message::Select(ids[2]));
        assert_eq!(a.files.picked, [ids[0], ids[2]]);
        let _ = a.update(Message::Select(ids[0]));
        assert_eq!(a.files.picked, [ids[2]]);
        with_mods(&mut a, keyboard::Modifiers::empty());
        let _ = a.update(Message::Select(ids[3]));
        assert!(a.files.picked.is_empty());
        assert_eq!(a.action_targets(), [ids[3]]);
    }

    #[test]
    fn shift_click_extends_selection() {
        let mut a = app();
        let ids = four(&mut a);
        let _ = a.update(Message::Select(ids[1]));
        with_mods(&mut a, keyboard::Modifiers::SHIFT);
        let _ = a.update(Message::Select(ids[3]));
        assert_eq!(a.files.picked, &ids[1..=3]);
        // Back past the anchor.
        let _ = a.update(Message::Select(ids[0]));
        assert_eq!(a.files.picked, &ids[0..=1]);
        // Enter does not open a multi-selection; Esc clears it.
        let _ = a.update(Message::OpenSelected);
        assert_eq!(a.screen, Screen::Library);
        let _ = a.update(Message::Escape);
        assert!(a.files.picked.is_empty());
        let _ = a.update(Message::Files(crate::files::Fx::SelectAll));
        assert_eq!(a.files.picked, ids);
    }

    #[test]
    fn delete_key_asks_before_trashing() {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Named(key::Named::Delete),
            modified_key: keyboard::Key::Named(key::Named::Delete),
            physical_key: key::Physical::Code(key::Code::Delete),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::empty(),
            text: None,
            repeat: false,
        });
        let msg = on_event(event, event::Status::Ignored, window::Id::unique()).unwrap();
        let mut a = app();
        let ids = four(&mut a);
        let _ = a.update(Message::Select(ids[1]));
        let _ = a.update(msg);
        assert!(matches!(
            &a.files.dialog,
            Some(crate::files::Dialog::Trash { ids: t, names, .. }) if *t == [ids[1]] && names[0] == "b"
        ));
        // Esc cancels without touching anything.
        let _ = a.update(Message::Escape);
        assert!(a.files.dialog.is_none());
    }

    fn done_op(id: i64) -> printdex_core::OpSummary {
        printdex_core::OpSummary {
            id,
            kind: printdex_core::OpKind::Rename,
            state: printdex_core::OpState::Done,
            names: vec!["Wall hook".into()],
            undoable: true,
        }
    }

    #[tokio::test]
    async fn undo_toast_undoes_last_action() {
        let mut a = app();
        let _ = a.update(Message::Engine(EngineEvent::FileOpDone(done_op(7))));
        let (op, seq) = a.files.toast.clone().unwrap();
        assert_eq!(crate::files::toast_text(&op), "Renamed to Wall hook");
        let _ = a.update(Message::Files(crate::files::Fx::Undo(7)));
        assert!(a.files.toast.is_none(), "the toast goes once undo starts");
        // An old timer does not hide a newer toast.
        let _ = a.update(Message::Engine(EngineEvent::FileOpDone(done_op(8))));
        let _ = a.update(Message::Files(crate::files::Fx::HideToast(seq)));
        assert!(a.files.toast.is_some());
    }

    #[tokio::test]
    async fn ctrl_z_undoes_last_file_action() {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character("z".into()),
            modified_key: keyboard::Key::Character("z".into()),
            physical_key: key::Physical::Code(key::Code::KeyZ),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::COMMAND,
            text: None,
            repeat: false,
        });
        let msg = on_event(event, event::Status::Ignored, window::Id::unique());
        assert!(matches!(
            msg,
            Some(Message::Files(crate::files::Fx::UndoLast))
        ));
        let mut a = app();
        let _ = a.update(Message::Engine(EngineEvent::FileOpDone(done_op(3))));
        let _ = a.update(msg.unwrap());
        assert!(a.files.toast.is_none(), "undid the toast's action");
    }

    #[test]
    fn rename_dialog_shows_name_taken_error() {
        let mut a = app();
        let ids = four(&mut a);
        let _ = a.update(Message::Select(ids[0]));
        let _ = a.update(Message::Files(crate::files::Fx::AskRename));
        let _ = a.update(Message::Files(crate::files::Fx::RenameInput("b".into())));
        let _ = a.update(Message::Files(crate::files::Fx::Done(Err(Arc::new(
            CoreError::NameTaken("b".into()),
        )))));
        match &a.files.dialog {
            Some(crate::files::Dialog::Rename { name, error, .. }) => {
                assert_eq!(name, "b");
                assert_eq!(
                    error.as_deref(),
                    Some("Something named b is already there.")
                );
            }
            other => panic!("{other:?}"),
        }
        // Typing clears the error.
        let _ = a.update(Message::Files(crate::files::Fx::RenameInput("b2".into())));
        assert!(matches!(
            &a.files.dialog,
            Some(crate::files::Dialog::Rename { error: None, .. })
        ));
    }

    #[tokio::test]
    async fn boot_without_libraries_shows_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).await.unwrap();
        let mut a = app();
        assert_eq!(a.boot, Boot::Opening);
        let _ = a.update(Message::Booted(Ok(engine)));
        assert_eq!(a.boot, Boot::Ready);
        assert!(a.search.loading);
        let _ = a.update(Message::Libraries(Ok((vec![], HashMap::new()))));
        assert!(a.libraries.is_empty());
        assert_eq!(a.total_models(), 0);
    }

    #[test]
    fn boot_failure_is_reported() {
        let mut a = app();
        let _ = a.update(Message::Booted(Err(Arc::new(CoreError::Cancelled))));
        assert_eq!(a.boot, Boot::Failed(PathBuf::from("/nonexistent")));
    }

    #[test]
    fn search_input_starts_query() {
        let mut a = app();
        let _ = a.update(Message::SearchChanged("dra".into()));
        assert_eq!(a.search.query.text, "dra");
        assert!(a.search.loading);
    }

    #[test]
    fn scan_progress_updates_sidebar() {
        let mut a = app();
        let lib = LibraryId::new();
        let _ = a.update(Message::Engine(EngineEvent::ScanProgress {
            library: lib,
            phase: ScanPhase::Fingerprinting,
            done: 3,
            total: 10,
        }));
        assert_eq!(a.scans[&lib].done, 3);
        let _ = a.update(Message::Engine(EngineEvent::ScanFinished {
            library: lib,
            summary: ScanSummary::default(),
        }));
        assert!(a.scans.is_empty());
    }

    #[test]
    fn scan_finished_reruns_current_query() {
        let mut a = app();
        let lib = LibraryId::new();
        let _ = a.update(Message::SearchChanged("orc".into()));
        deliver(&mut a, vec![hit("Orc")]);
        let _ = a.update(Message::SelectScope(Scope::Library(lib)));
        deliver(&mut a, vec![]);
        assert!(!a.search.loading);
        let _ = a.update(Message::Engine(EngineEvent::ScanFinished {
            library: lib,
            summary: ScanSummary::default(),
        }));
        assert!(a.search.loading);
        assert_eq!(
            (a.search.query.text.as_str(), &a.search.query.scope),
            ("orc", &Scope::Library(lib))
        );
    }

    #[test]
    fn selection_survives_refresh() {
        let mut a = app();
        let (x, y) = (hit("x"), hit("y"));
        deliver(&mut a, vec![x.clone(), y.clone()]);
        let _ = a.update(Message::Select(y.id));
        deliver(&mut a, vec![y.clone()]);
        assert_eq!(a.selected, Some(y.id));
        deliver(&mut a, vec![x]);
        assert_eq!(a.selected, None);
    }

    #[test]
    fn add_library_error_is_shown() {
        let mut a = app();
        let _ = a.update(Message::LibraryPicked(None));
        assert_eq!(a.error, None);
        let _ = a.update(Message::LibraryAdded(Err(Arc::new(
            CoreError::LibraryOverlaps {
                existing: PathBuf::from("/srv/models"),
            },
        ))));
        let e = a.error.clone().unwrap();
        assert!(e.contains("/srv/models"), "{e}");
        let _ = a.update(Message::DismissError);
        assert_eq!(a.error, None);
    }

    #[test]
    fn selecting_library_scopes_search() {
        let mut a = app();
        let lib = LibraryId::new();
        deliver(&mut a, vec![]);
        let _ = a.update(Message::SelectScope(Scope::Library(lib)));
        assert_eq!(a.search.query.scope, Scope::Library(lib));
        assert!(a.search.loading);
    }

    #[test]
    fn removed_scope_falls_back_to_all() {
        let mut a = app();
        let _ = a.update(Message::SelectScope(Scope::Library(LibraryId::new())));
        deliver(&mut a, vec![]);
        let _ = a.update(Message::Libraries(Ok((vec![], HashMap::new()))));
        assert_eq!(a.search.query.scope, Scope::All);
    }

    #[test]
    fn arrows_move_selection_through_the_grid() {
        let mut a = app();
        let hits: Vec<_> = (0..12).map(|i| hit(&format!("m{i}"))).collect();
        deliver(&mut a, hits.clone());
        assert_eq!(a.metrics().columns, 5);
        let _ = a.update(Message::Nav(Move::Right));
        assert_eq!(a.selected, Some(hits[0].id));
        let _ = a.update(Message::Nav(Move::Down));
        assert_eq!(a.selected, Some(hits[5].id));
        let _ = a.update(Message::Nav(Move::End));
        assert_eq!(a.selected, Some(hits[11].id));
    }

    #[test]
    fn escape_clears_search_then_selection() {
        let mut a = app();
        let h = hit("orc");
        let _ = a.update(Message::SearchChanged("orc".into()));
        deliver(&mut a, vec![h.clone()]);
        let _ = a.update(Message::Select(h.id));
        let _ = a.update(Message::Escape);
        assert_eq!(a.search.query.text, "");
        deliver(&mut a, vec![h.clone()]);
        assert_eq!(a.selected, Some(h.id));
        let _ = a.update(Message::Escape);
        assert_eq!(a.selected, None);
    }

    #[test]
    fn follows_system_dark_mode() {
        let mut a = app();
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Dark));
        assert_eq!(a.mode(), Mode::Dark);
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Light));
        assert_eq!(a.mode(), Mode::Light);
    }

    #[test]
    fn fixed_mode_ignores_system() {
        let mut a = App::new(Options {
            mode: Some(Mode::Dark),
            ..Options::in_memory(PathBuf::from("/nonexistent"))
        });
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Light));
        assert_eq!(a.mode(), Mode::Dark);
        // The override is not a setting.
        assert_eq!(a.settings.mode, ThemeMode::System);
    }

    #[test]
    fn enter_opens_selected_model() {
        let mut a = app();
        let h = hit("orc");
        deliver(&mut a, vec![h.clone()]);
        let _ = a.update(Message::OpenSelected);
        assert_eq!(a.screen, Screen::Library);
        let _ = a.update(Message::Select(h.id));
        let _ = a.update(Message::OpenSelected);
        assert_eq!(a.screen, Screen::Model(h.id));
    }

    #[test]
    fn escape_returns_to_library() {
        let mut a = app();
        let h = hit("orc");
        let _ = a.update(Message::SearchChanged("orc".into()));
        deliver(&mut a, vec![h.clone()]);
        let _ = a.update(Message::OpenModel(h.id));
        let _ = a.update(Message::Escape);
        assert_eq!(a.screen, Screen::Library);
        // Back keeps the search and the selection.
        assert_eq!(a.search.query.text, "orc");
        assert_eq!(a.selected, Some(h.id));
        let _ = a.update(Message::OpenSettings);
        let _ = a.update(Message::Escape);
        assert_eq!(a.screen, Screen::Library);
    }

    use crate::org::{CollectionEdit, Lists};
    use printdex_core::{CollectionId, CollectionInfo, ModelData, SortOrder, TagCount};

    /// Opens a model and delivers its data.
    fn open_with_data(a: &mut App, data: ModelData) -> ModelId {
        let h = hit("orc");
        deliver(a, vec![h.clone()]);
        let _ = a.update(Message::OpenModel(h.id));
        let _ = a.update(Message::Org(Org::DataLoaded(
            h.id,
            Ok(Some(Box::new(data))),
        )));
        h.id
    }

    #[test]
    fn selecting_collection_scopes_search_in_collection_order() {
        let mut a = app();
        deliver(&mut a, vec![]);
        let c = CollectionId::new();
        let _ = a.update(Message::SelectScope(Scope::Collection(c)));
        assert_eq!(a.search.query.scope, Scope::Collection(c));
        assert_eq!(a.search.query.sort, SortOrder::Collection);
        assert!(a.search.loading);
        // Elsewhere the saved sort applies again.
        deliver(&mut a, vec![]);
        let _ = a.update(Message::SelectScope(Scope::All));
        assert_eq!(a.search.query.sort, a.settings.sort);
    }

    #[test]
    fn selecting_tag_scopes_search() {
        let mut a = app();
        deliver(&mut a, vec![]);
        let _ = a.update(Message::SelectScope(Scope::Tag("pla".into())));
        assert_eq!(a.search.query.scope, Scope::Tag("pla".into()));
        assert!(a.search.loading);
    }

    #[test]
    fn deleted_collection_scope_falls_back_to_all() {
        let mut a = app();
        let c = CollectionId::new();
        let _ = a.update(Message::SelectScope(Scope::Collection(c)));
        deliver(&mut a, vec![]);
        let _ = a.update(Message::Org(Org::Lists(Ok(Lists::default()))));
        assert_eq!(a.search.query.scope, Scope::All);
    }

    #[test]
    fn model_data_changed_reloads_open_model() {
        let mut a = app();
        let id = open_with_data(&mut a, ModelData::default());
        a.thumbs.ready(
            ThumbTarget::Model(id),
            "/x/a.png".into(),
            printdex_core::ThumbKind::Render,
        );
        let _ = a.update(Message::Engine(EngineEvent::ModelDataChanged(id)));
        // Cards rerun (tags match) and the preview is asked again (material).
        assert!(a.search.loading);
        assert!(a.thumbs.pending(ThumbTarget::Model(id)));
        assert!(a.thumbs.get(ThumbTarget::Model(id)).is_some());
    }

    #[test]
    fn data_for_another_model_is_ignored() {
        let mut a = app();
        let id = open_with_data(&mut a, ModelData::default());
        let other = ModelId::new();
        let data = ModelData {
            notes: "other".into(),
            ..ModelData::default()
        };
        let _ = a.update(Message::Org(Org::DataLoaded(
            other,
            Ok(Some(Box::new(data))),
        )));
        assert_eq!(a.org.data_for(id).map(|d| d.notes.as_str()), Some(""));
    }

    #[test]
    fn t_opens_tag_input_only_in_model_view() {
        let mut a = app();
        let _ = a.update(Message::Org(Org::StartTag));
        assert_eq!(a.org.tag_input, None);
        open_with_data(&mut a, ModelData::default());
        let _ = a.update(Message::Org(Org::StartTag));
        assert_eq!(a.org.tag_input.as_deref(), Some(""));
        // Enter with text keeps the field open for the next tag.
        let _ = a.update(Message::Org(Org::TagInput("pla".into())));
        let _ = a.update(Message::Org(Org::SubmitTag));
        assert_eq!(a.org.tag_input.as_deref(), Some(""));
        // Enter on an empty field closes it.
        let _ = a.update(Message::Org(Org::SubmitTag));
        assert_eq!(a.org.tag_input, None);
    }

    #[test]
    fn n_starts_editing_notes_and_save_shows_them() {
        let mut a = app();
        let data = ModelData {
            notes: "Print *flat*".into(),
            ..ModelData::default()
        };
        let id = open_with_data(&mut a, data);
        assert!(!a.org.notes_md.is_empty());
        let _ = a.update(Message::Org(Org::StartNotes));
        let text = a.org.notes_edit.as_ref().map(text_editor_text);
        assert_eq!(text.as_deref(), Some("Print *flat*"));
        a.org.notes_edit = Some(iced::widget::text_editor::Content::with_text("Brim\n"));
        let _ = a.update(Message::Org(Org::SaveNotes));
        assert!(a.org.notes_edit.is_none());
        assert_eq!(a.org.data_for(id).map(|d| d.notes.as_str()), Some("Brim"));
    }

    fn text_editor_text(c: &iced::widget::text_editor::Content) -> String {
        c.text().trim_end_matches('\n').to_owned()
    }

    #[test]
    fn escape_cancels_editing_before_leaving() {
        let mut a = app();
        open_with_data(&mut a, ModelData::default());
        let _ = a.update(Message::Org(Org::StartNotes));
        let _ = a.update(Message::Escape);
        assert!(a.org.notes_edit.is_none());
        assert!(
            matches!(a.screen, Screen::Model(_)),
            "first Esc only cancels"
        );
        let _ = a.update(Message::Escape);
        assert_eq!(a.screen, Screen::Library);
    }

    #[test]
    fn invalid_source_is_refused_in_words() {
        let mut a = app();
        open_with_data(&mut a, ModelData::default());
        let _ = a.update(Message::Org(Org::StartSource));
        let _ = a.update(Message::Org(Org::SourceInput("ftp://x".into())));
        let _ = a.update(Message::Org(Org::SaveSource));
        assert!(a.error.is_some());
        assert!(
            a.org.source_edit.is_some(),
            "the field stays open to fix it"
        );
    }

    #[test]
    fn collection_delete_needs_confirmation() {
        let mut a = app();
        let c = CollectionId::new();
        let _ = a.update(Message::Org(Org::Lists(Ok(Lists {
            collections: vec![CollectionInfo {
                id: c,
                name: "Desk".into(),
                count: 2,
            }],
            tags: vec![TagCount {
                name: "pla".into(),
                count: 1,
            }],
            ..Lists::default()
        }))));
        let _ = a.update(Message::SelectScope(Scope::Collection(c)));
        // A stray confirm does nothing.
        let _ = a.update(Message::Org(Org::ConfirmDeleteCollection(c)));
        assert_eq!(a.org.collection_edit, None);
        let _ = a.update(Message::Org(Org::AskDeleteCollection(c)));
        assert_eq!(
            a.org.collection_edit,
            Some(CollectionEdit::ConfirmDelete(c))
        );
        let _ = a.update(Message::Escape);
        assert_eq!(a.org.collection_edit, None);
    }

    fn detail(id: ModelId, parts: &[&str]) -> ModelDetail {
        ModelDetail {
            id,
            library_id: LibraryId::new(),
            name: "Bin".into(),
            rel_path: "bins/bin".into(),
            kind: ModelKind::Folder,
            number: Some(1),
            created_at: 0,
            missing_since: None,
            parts: parts
                .iter()
                .map(|p| PartInfo {
                    id: printdex_core::PartId::new(),
                    rel_path: format!("bins/bin/{p}"),
                    format: printdex_core::PrintableFormat::Stl,
                    size: 10,
                    dims: None,
                    triangles: None,
                })
                .collect(),
            assets: vec![],
        }
    }

    fn library(name: &str) -> Library {
        Library {
            id: LibraryId::new(),
            path: PathBuf::from("/srv").join(name),
            display_name: name.into(),
            kind: printdex_core::LibraryKind::Local,
            change_mode: printdex_core::ChangeMode::Auto,
            sidecar_enabled: false,
            last_full_scan_at: None,
        }
    }

    #[test]
    fn monitor_events_mark_libraries_offline_and_back() {
        let mut a = app();
        let lib = library("NAS");
        a.libraries.push(lib.clone());
        let _ = a.update(Message::Engine(EngineEvent::LibraryOffline(lib.id)));
        assert!(a.offline.contains_key(&lib.id));
        let _ = a.update(Message::Engine(EngineEvent::LibraryOnline(lib.id)));
        assert!(!a.offline.contains_key(&lib.id));
    }

    #[test]
    fn offline_library_is_marked_and_kept() {
        let mut a = app();
        let lib = library("NAS");
        let h = ModelHit {
            library_id: lib.id,
            ..hit("orc")
        };
        a.libraries.push(lib.clone());
        deliver(&mut a, vec![h.clone()]);
        let _ = a.update(Message::Status(lib.id, Ok(LibraryStatus::Offline), true));
        assert!(a.offline.contains_key(&lib.id));
        assert!(a.scans.is_empty(), "no scan of an offline library");
        assert_eq!(a.libraries.len(), 1);
        assert_eq!(a.search.hits, vec![h]);
        assert_eq!(a.error, None);
    }

    #[test]
    fn rescan_of_offline_library_shows_offline_not_error() {
        let mut a = app();
        let lib = library("NAS");
        a.libraries.push(lib.clone());
        let _ = a.update(Message::ScanDone(
            lib.id,
            Err(Arc::new(CoreError::LibraryOffline(lib.path.clone()))),
        ));
        assert_eq!(a.error, None);
        assert!(a.offline.contains_key(&lib.id));
        let other = library("USB");
        a.libraries.push(other.clone());
        let _ = a.update(Message::Engine(EngineEvent::ScanFailed {
            library: other.id,
            error: Arc::new(CoreError::LibraryOffline(other.path.clone())),
        }));
        assert_eq!(a.error, None);
        assert!(a.offline.contains_key(&other.id));
    }

    #[test]
    fn retry_brings_library_back() {
        let mut a = app();
        let lib = library("NAS");
        a.libraries.push(lib.clone());
        let _ = a.update(Message::Status(lib.id, Ok(LibraryStatus::Offline), false));
        let since = a.offline[&lib.id].clone();
        // A second offline report keeps the first time.
        let _ = a.update(Message::Status(lib.id, Ok(LibraryStatus::Offline), false));
        assert_eq!(a.offline[&lib.id], since);
        let _ = a.update(Message::Status(lib.id, Ok(LibraryStatus::Online), true));
        assert!(!a.offline.contains_key(&lib.id));
        assert!(a.scans.contains_key(&lib.id));
    }

    #[test]
    fn remove_requires_confirmation() {
        let mut a = app();
        let lib = library("NAS");
        a.libraries.push(lib.clone());
        // Without the confirmation showing, nothing is removed.
        let task = a.update(Message::ConfirmRemove(lib.id));
        assert_eq!(task.units(), 0);
        let _ = a.update(Message::AskRemove(lib.id));
        assert_eq!(a.confirm_remove, Some(lib.id));
        let _ = a.update(Message::Escape);
        assert_eq!(a.confirm_remove, None);
        assert_eq!(a.screen, Screen::Library);
        let _ = a.update(Message::AskRemove(lib.id));
        let _ = a.update(Message::ConfirmRemove(LibraryId::new()));
        assert_eq!(a.confirm_remove, None);
        // The engine's event drops it from the sidebar.
        let _ = a.update(Message::Engine(EngineEvent::LibraryRemoved(lib.id)));
        assert!(a.libraries.is_empty());
    }

    #[test]
    fn rename_library_updates_sidebar() {
        let mut a = app();
        let lib = library("NAS");
        a.libraries.push(lib.clone());
        let _ = a.update(Message::StartRename(lib.id));
        assert_eq!(a.renaming, Some((lib.id, "NAS".into())));
        let _ = a.update(Message::RenameInput("NAS prints".into()));
        let _ = a.update(Message::CommitRename);
        assert_eq!(a.renaming, None);
        let renamed = Library {
            display_name: "NAS prints".into(),
            ..lib.clone()
        };
        let _ = a.update(Message::Engine(EngineEvent::LibraryRenamed(renamed)));
        assert_eq!(a.library(lib.id).unwrap().display_name, "NAS prints");
    }

    #[test]
    fn changing_colour_theme_saves_settings() {
        let dir = tempfile::tempdir().unwrap();
        let mut a = App::new(Options {
            config_dir: Some(dir.path().to_path_buf()),
            ..Options::in_memory(PathBuf::from("/nonexistent"))
        });
        let s = Settings {
            color_theme: ColorTheme::Glacier,
            ..a.settings
        };
        let task = a.update(Message::SettingsChanged(s));
        assert_eq!(a.color_theme(), ColorTheme::Glacier);
        assert_eq!(
            a.target_palette(),
            Palette::new(ColorTheme::Glacier, a.mode(), Surface::Solid)
        );
        assert!(task.units() > 0, "a save was started");
        // Without a config folder nothing is written.
        let mut b = app();
        assert_eq!(b.update(Message::SettingsChanged(s)).units(), 0);
    }

    #[test]
    fn system_mode_follows_os() {
        let mut a = app();
        let s = Settings {
            mode: ThemeMode::Dark,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Light));
        assert_eq!(a.mode(), Mode::Dark);
        let s = Settings {
            mode: ThemeMode::System,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        assert_eq!(a.mode(), Mode::Light);
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Dark));
        assert_eq!(a.mode(), Mode::Dark);
    }

    #[test]
    fn chips_combine_with_text() {
        let mut a = app();
        let _ = a.update(Message::SearchChanged("bin".into()));
        let _ = a.update(Message::ToggleChip(Chip::Format(
            printdex_core::PrintableFormat::ThreeMf,
        )));
        assert_eq!(a.search.query.text, "bin");
        assert_eq!(
            a.search.query.filter.formats,
            vec![printdex_core::PrintableFormat::ThreeMf]
        );
        assert!(a.search.loading);
        let _ = a.update(Message::ToggleChip(Chip::Format(
            printdex_core::PrintableFormat::ThreeMf,
        )));
        assert!(a.search.query.filter.is_empty());
    }

    #[test]
    fn chips_survive_scope_change() {
        let mut a = app();
        let _ = a.update(Message::ToggleChip(Chip::MultiPart(true)));
        let _ = a.update(Message::SelectScope(Scope::Library(LibraryId::new())));
        assert_eq!(a.chips, vec![Chip::MultiPart(true)]);
        assert_eq!(a.search.query.filter.multi_part, Some(true));
    }

    #[test]
    fn theme_change_cross_fades() {
        let mut a = app();
        let before = a.palette();
        let s = Settings {
            color_theme: ColorTheme::RosePetg,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        // Right after the change the old colours still show...
        assert_eq!(a.palette().t.accent, before.t.accent);
        assert!(a.motion.animating(a.motion.now));
        // ...and the new ones once the fade is over.
        let _ = a.update(Message::Frame(a.motion.now + crate::motion::THEME));
        assert_eq!(a.palette(), a.target_palette());
    }

    #[test]
    fn list_mode_moves_by_row() {
        let mut a = app();
        let hits: Vec<_> = (0..12).map(|i| hit(&format!("m{i}"))).collect();
        deliver(&mut a, hits.clone());
        let s = Settings {
            view: ViewMode::List,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        let _ = a.update(Message::Nav(Move::Down));
        let _ = a.update(Message::Nav(Move::Down));
        assert_eq!(a.selected, Some(hits[1].id));
        let _ = a.update(Message::OpenSelected);
        assert_eq!(a.screen, Screen::Model(hits[1].id));
    }

    #[test]
    fn scroll_requests_visible_models() {
        let mut a = app();
        let hits: Vec<ModelHit> = (0..300).map(|i| hit(&format!("m{i:03}"))).collect();
        deliver(&mut a, hits.clone());
        let first = ThumbTarget::Model(hits[0].id);
        let last = ThumbTarget::Model(hits[299].id);
        let seen = a.visible_thumbnails();
        assert_eq!(seen.first(), Some(&first), "reading order");
        assert!(!seen.contains(&last));
        // Scrolled to the end.
        let m = a.metrics();
        let _ = a.update(Message::Scrolled(GridViewport {
            offset: m.rows(300) as f32 * m.stride() + 2.0 * GRID_PAD_Y - 600.0,
            height: 600.0,
        }));
        let seen = a.visible_thumbnails();
        assert!(seen.contains(&last));
        assert!(!seen.contains(&first));
        assert!(seen.len() < 60, "only what is on screen ({})", seen.len());
    }

    #[test]
    fn ready_event_swaps_placeholder() {
        let mut a = app();
        let hits = vec![hit("a"), hit("b")];
        deliver(&mut a, hits.clone());
        let t = ThumbTarget::Model(hits[0].id);
        assert!(a.thumbs.pending(t));
        let _ = a.update(Message::Engine(EngineEvent::ThumbnailReady {
            target: t,
            path: "/cache/p00.png".into(),
            kind: printdex_core::ThumbKind::Render,
            print_in: None,
        }));
        assert!(matches!(
            a.thumbs.get(t),
            Some(crate::thumbs::Thumb::Ready { .. })
        ));
        let wanted = a.thumbs.request_for(a.visible_thumbnails());
        assert_eq!(wanted, Some(vec![ThumbTarget::Model(hits[1].id)]));
    }

    #[test]
    fn missing_thumb_keeps_placeholder() {
        let mut a = app();
        let hits = vec![hit("a")];
        deliver(&mut a, hits.clone());
        let t = ThumbTarget::Model(hits[0].id);
        let _ = a.update(Message::Engine(EngineEvent::ThumbnailFailed {
            target: t,
            reason: printdex_core::ThumbFailure::TooLarge,
        }));
        assert!(matches!(
            a.thumbs.get(t),
            Some(crate::thumbs::Thumb::Failed(
                printdex_core::ThumbFailure::TooLarge
            ))
        ));
        assert!(!a.thumbs.pending(t), "not asked for again");
    }

    #[test]
    fn model_view_requests_parts() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        assert!(a.visible_thumbnails().is_empty());
        let d = detail(id, &["a.stl", "b.stl", "c.stl"]);
        let parts: Vec<ThumbTarget> = d.parts.iter().map(|p| ThumbTarget::Part(p.id)).collect();
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(d))));
        let _ = a.update(Message::SelectPart(2));
        let seen = a.visible_thumbnails();
        assert_eq!(seen[0], parts[2], "selected part first");
        for p in &parts {
            assert!(seen.contains(p));
        }
    }

    #[test]
    fn forget_requires_confirmation() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        let d = ModelDetail {
            missing_since: Some(1),
            ..detail(id, &["a.stl"])
        };
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(d))));
        // Without asking first, Forget does nothing.
        let _ = a.update(Message::Org(Org::ConfirmForget(id)));
        assert!(!a.org.confirm_forget);
        let _ = a.update(Message::Org(Org::AskForget));
        assert!(a.org.confirm_forget);
        // Esc backs out of the question, not out of the model.
        let _ = a.update(Message::Escape);
        assert!(!a.org.confirm_forget);
        assert!(matches!(a.model, Some(ModelView::Loaded { .. })));
        let _ = a.update(Message::Org(Org::AskForget));
        let _ = a.update(Message::Org(Org::ConfirmForget(id)));
        assert!(!a.org.confirm_forget, "asked once, done");
    }

    #[test]
    fn forgotten_model_closes_its_view() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(detail(id, &["a.stl"])))));
        let _ = a.update(Message::Engine(EngineEvent::ModelForgotten(id)));
        assert!(a.model.is_none());
    }

    #[test]
    fn change_mode_saved_per_library() {
        let mut a = app();
        let lib = library("NAS");
        let other = library("Desk");
        a.libraries.push(lib.clone());
        a.libraries.push(other.clone());
        let changed = Library {
            change_mode: printdex_core::ChangeMode::Every(60),
            kind: printdex_core::LibraryKind::Network,
            ..lib.clone()
        };
        let _ = a.update(Message::Engine(EngineEvent::LibraryChanged(changed)));
        assert_eq!(
            a.libraries[0].change_mode,
            printdex_core::ChangeMode::Every(60)
        );
        assert_eq!(a.libraries[1].change_mode, printdex_core::ChangeMode::Auto);
        assert_eq!(
            crate::view::settings::change_summary(&a.libraries[0]),
            "Network share, checked every 60 minutes"
        );
        assert_eq!(
            crate::view::settings::change_summary(&a.libraries[1]),
            "Local folder, watched for changes"
        );
        let shares = Library {
            kind: printdex_core::LibraryKind::Network,
            ..other
        };
        assert_eq!(
            crate::view::settings::change_summary(&shares),
            "Network share, checked every 15 minutes"
        );
        assert_eq!(crate::view::settings::mode_choices().len(), 6);
    }

    #[test]
    fn opening_model_loads_detail() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        assert_eq!(a.model, Some(ModelView::Loading(id)));
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(detail(id, &["a.stl"])))));
        assert!(matches!(a.model, Some(ModelView::Loaded { part: 0, .. })));
    }

    #[test]
    fn detail_for_other_model_is_ignored() {
        let mut a = app();
        let (first, second) = (ModelId::new(), ModelId::new());
        let _ = a.update(Message::OpenModel(first));
        let _ = a.update(Message::OpenModel(second));
        let _ = a.update(Message::ModelLoaded(
            first,
            Ok(Some(detail(first, &["a.stl"]))),
        ));
        assert_eq!(a.model, Some(ModelView::Loading(second)));
    }

    #[test]
    fn missing_model_shows_notice() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(detail(id, &["a.stl"])))));
        // A rescan removed it while it was on screen.
        let _ = a.update(Message::ModelLoaded(id, Ok(None)));
        assert_eq!(a.model, Some(ModelView::Missing(id)));
        let _ = a.update(Message::Escape);
        assert_eq!(a.screen, Screen::Library);
        assert_eq!(a.model, None);
    }

    #[test]
    fn selecting_part_updates_caption() {
        let mut a = app();
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        let _ = a.update(Message::ModelLoaded(
            id,
            Ok(Some(detail(id, &["a.stl", "b.stl", "c.stl"]))),
        ));
        assert_eq!(a.current_part().unwrap().rel_path, "bins/bin/a.stl");
        let _ = a.update(Message::SelectPart(2));
        assert_eq!(a.current_part().unwrap().rel_path, "bins/bin/c.stl");
        let _ = a.update(Message::Nav(Move::Left));
        assert_eq!(a.current_part().unwrap().rel_path, "bins/bin/b.stl");
        let _ = a.update(Message::SelectPart(9));
        assert_eq!(a.current_part().unwrap().rel_path, "bins/bin/b.stl");
        // A reload keeps the selected part.
        let _ = a.update(Message::ModelLoaded(
            id,
            Ok(Some(detail(id, &["a.stl", "b.stl", "c.stl"]))),
        ));
        assert_eq!(a.current_part().unwrap().rel_path, "bins/bin/b.stl");
    }

    #[test]
    fn ctrl_comma_opens_settings() {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(",".into()),
            modified_key: keyboard::Key::Character(",".into()),
            physical_key: key::Physical::Code(key::Code::Comma),
            location: keyboard::Location::Standard,
            modifiers: keyboard::Modifiers::COMMAND,
            text: None,
            repeat: false,
        });
        let msg = on_event(event, event::Status::Ignored, window::Id::unique());
        assert!(matches!(msg, Some(Message::OpenSettings)));
    }

    #[test]
    fn settings_mode_wins_over_system() {
        let mut a = app();
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Dark));
        let s = Settings {
            mode: ThemeMode::Light,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        assert_eq!(a.mode(), Mode::Light);
    }

    #[test]
    fn changing_sort_reruns_search() {
        let mut a = app();
        deliver(&mut a, vec![]);
        let s = Settings {
            sort: printdex_core::SortOrder::RecentlyAdded,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        assert!(a.search.loading);
        assert_eq!(a.search.query.sort, printdex_core::SortOrder::RecentlyAdded);
    }

    #[test]
    fn glass_needs_transparent_window_and_translucency() {
        let mut a = App::new(Options {
            glass_window: true,
            ..Options::in_memory(PathBuf::from("/nonexistent"))
        });
        assert_eq!(a.surface(), Surface::Glass);
        let s = Settings {
            translucency: Translucency::Off,
            ..a.settings
        };
        let _ = a.update(Message::SettingsChanged(s));
        assert_eq!(a.surface(), Surface::Solid);
        assert_eq!(app().surface(), Surface::Solid);
    }

    fn key(c: &str, modifiers: keyboard::Modifiers) -> Option<Message> {
        let event = Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(c.into()),
            modified_key: keyboard::Key::Character(c.into()),
            physical_key: key::Physical::Code(key::Code::KeyL),
            location: keyboard::Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        });
        on_event(event, event::Status::Ignored, window::Id::unique())
    }

    fn petg_data() -> printdex_core::ModelData {
        use printdex_core::{Material, PrintSettings, Recipe};
        printdex_core::ModelData {
            recipe: Some(Recipe {
                settings: PrintSettings {
                    material: Some(Material::Petg),
                    layer_mm: Some(0.2),
                    printer: Some("Prusa MK4".into()),
                    ..PrintSettings::default()
                },
                ..Recipe::default()
            }),
            ..printdex_core::ModelData::default()
        }
    }

    /// An app showing a model whose recipe is PETG at 0.20 mm.
    fn model_open(a: &mut App) -> ModelId {
        let id = ModelId::new();
        let _ = a.update(Message::OpenModel(id));
        let _ = a.update(Message::ModelLoaded(id, Ok(Some(detail(id, &["a.stl"])))));
        let _ = a.update(Message::Org(crate::org::Org::DataLoaded(
            id,
            Ok(Some(Box::new(petg_data()))),
        )));
        id
    }

    #[test]
    fn l_opens_log_form() {
        assert!(matches!(
            key("l", keyboard::Modifiers::empty()),
            Some(Message::Prints(Pr::OpenLog))
        ));
        let mut a = app();
        model_open(&mut a);
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let f = a.prints.form.as_ref().expect("form open");
        assert_eq!(f.mode, crate::log_form::FormMode::Log);
        assert!(f.prefilled);
        assert_eq!(f.material, "PETG");
        assert_eq!(f.layer, "0.20");
    }

    #[test]
    fn l_does_nothing_in_the_library() {
        let mut a = app();
        let _ = a.update(Message::Prints(Pr::OpenLog));
        assert!(a.prints.form.is_none());
    }

    #[test]
    fn cancel_discards() {
        let mut a = app();
        model_open(&mut a);
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let _ = a.update(Message::Prints(Pr::Input(
            crate::prints::Input::Printer,
            "Voron".into(),
        )));
        // Esc closes the form first, and stays in the model view.
        let _ = a.update(Message::Escape);
        assert!(a.prints.form.is_none());
        assert!(matches!(a.screen, Screen::Model(_)));
        let _ = a.update(Message::Prints(Pr::OpenLog));
        assert_eq!(a.prints.form.as_ref().unwrap().printer, "Prusa MK4");
    }

    #[tokio::test]
    async fn ctrl_enter_saves_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).await.unwrap();
        let mut a = app();
        let _ = a.update(Message::Booted(Ok(engine)));
        model_open(&mut a);
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let _ = a.update(Message::SaveShortcut);
        assert!(a.prints.saving, "save dispatched");
        // A second Ctrl+Enter while saving does nothing.
        let _ = a.update(Message::SaveShortcut);
        assert!(a.prints.saving);
    }

    #[tokio::test]
    async fn invalid_form_shows_error_and_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(dir.path()).await.unwrap();
        let mut a = app();
        let _ = a.update(Message::Booted(Ok(engine)));
        model_open(&mut a);
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let _ = a.update(Message::Prints(Pr::Input(
            crate::prints::Input::Bed,
            "180".into(),
        )));
        let _ = a.update(Message::SaveShortcut);
        assert!(!a.prints.saving);
        let f = a.prints.form.as_ref().unwrap();
        assert!(crate::prints::has_error(f, crate::log_form::Field::Bed));
    }

    #[tokio::test]
    async fn save_closes_form_and_reloads() {
        let mut a = app();
        model_open(&mut a);
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let attempt = printdex_core::AttemptId::new();
        let _ = a.update(Message::Prints(Pr::Saved(Ok(Some((attempt, 4))))));
        assert!(a.prints.form.is_none());
        assert_eq!(a.prints.fresh, Some(attempt));
        assert_eq!(a.prints.toast.as_deref(), Some("Attempt 4 saved"));
    }

    #[test]
    fn dropped_image_attaches_photo() {
        let mut a = app();
        model_open(&mut a);
        // Nothing to attach to without the form.
        let _ = a.update(Message::Prints(Pr::FileDropped("/tmp/a.png".into())));
        assert!(a.prints.form.is_none());
        let _ = a.update(Message::Prints(Pr::OpenLog));
        let _ = a.update(Message::Prints(Pr::FileDropped("/tmp/a.png".into())));
        let _ = a.update(Message::Prints(Pr::FileDropped("/tmp/model.stl".into())));
        assert_eq!(
            a.prints.form.as_ref().unwrap().photos,
            vec![PathBuf::from("/tmp/a.png")]
        );
    }

    #[test]
    fn material_chip_filters() {
        use crate::filters::MatChip;
        let mut a = app();
        let _ = a.update(Message::ToggleChip(Chip::Material(MatChip::Petg)));
        assert_eq!(
            a.search.query.filter.materials,
            vec![printdex_core::Material::Petg]
        );
        assert!(a.search.loading);
    }
}
