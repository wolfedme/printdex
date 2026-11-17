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
    CoreError, Engine, EngineEvent, Library, LibraryId, ModelDetail, ModelHit, ModelId, PartInfo,
    ScanPhase, ScanSummary,
};
use tokio_util::sync::CancellationToken;

use crate::filters::{self, Chip};
use crate::grid::GridMetrics;
use crate::i18n::{Arg, t, t_args};
use crate::nav::{self, Move};
use crate::search::{SEARCH_LIMIT, SearchState, Ticket};
use crate::settings::{Settings, ThemeMode, Translucency, ViewMode};
use crate::theme::{self, ColorTheme, Mode, Palette, Surface};

/// Widget ids the update logic talks to.
pub fn search_id() -> Id {
    Id::new("search")
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

/// Startup options from the command line and the settings file.
#[derive(Clone, Debug)]
pub struct Options {
    pub data_dir: PathBuf,
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
            data_dir,
            config_dir: None,
            mode: None,
            settings: Settings::default(),
            glass_window: false,
        }
    }
}

/// The model view's content.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub settings: Settings,
    /// The OS's light/dark preference, used when the mode is System.
    pub system_mode: Mode,
    pub window: Size,
    pub viewport: Option<GridViewport>,
    cancel: CancellationToken,
    started: std::time::Instant,
}

/// Libraries and their present-model counts.
pub type LibrarySnapshot = (Vec<Library>, HashMap<LibraryId, u64>);

#[derive(Clone, Debug)]
pub enum Message {
    Booted(Result<Engine, Arc<CoreError>>),
    Libraries(Result<LibrarySnapshot, Arc<CoreError>>),
    Engine(EngineEvent),
    SearchChanged(String),
    Results(Ticket, Result<(Vec<ModelHit>, u64), Arc<CoreError>>),
    SelectScope(Option<LibraryId>),
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
    Launched(Result<(), Arc<std::io::Error>>),
    SettingsChanged(Settings),
    SettingsSaved(Result<(), Arc<std::io::Error>>),
}

impl App {
    /// State before the engine is open.
    pub fn new(options: Options) -> App {
        let settings = options.settings;
        let mut search = SearchState::default();
        search.query.sort = settings.sort;
        App {
            options,
            engine: None,
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
            chips: Vec::new(),
            filter_menu: false,
            settings,
            system_mode: Mode::default(),
            window: INITIAL_WINDOW,
            viewport: None,
            cancel: CancellationToken::new(),
            started: std::time::Instant::now(),
        }
    }

    /// Initial state and the tasks that open the engine and read the OS theme.
    pub fn boot(options: Options) -> (App, Task<Message>) {
        let dir = options.data_dir.clone();
        let open = Task::perform(
            async move { Engine::open(&dir).await.map_err(Arc::new) },
            Message::Booted,
        );
        let mode = iced::system::theme().map(Message::SystemMode);
        (App::new(options), Task::batch([open, mode]))
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

    pub fn palette(&self) -> Palette {
        Palette::new(self.color_theme(), self.mode(), self.surface())
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
        match message {
            Message::Booted(Ok(engine)) => {
                self.engine = Some(engine);
                self.boot = Boot::Ready;
                let ticket = self.search.refresh();
                Task::batch([self.load_libraries(), self.run_search(ticket)])
            }
            Message::Booted(Err(e)) => {
                tracing::error!(error = %e, "could not open engine");
                self.boot = Boot::Failed(self.options.data_dir.clone());
                Task::none()
            }
            Message::Libraries(Ok((libraries, counts))) => {
                self.libraries = libraries;
                self.counts = counts;
                if let Some(scope) = self.search.query.scope
                    && self.library(scope).is_none()
                {
                    return self.set_scope(None);
                }
                Task::none()
            }
            Message::Libraries(Err(e)) | Message::Results(_, Err(e)) => {
                self.show_error(&e);
                self.search.loading = false;
                Task::none()
            }
            Message::Engine(event) => self.on_engine_event(event),
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
            Message::SelectScope(scope) => self.set_scope(scope),
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
                    Err(e) => {
                        self.show_error(&e);
                        Task::none()
                    }
                }
            }
            Message::Rescan => {
                let ids: Vec<LibraryId> = match self.search.query.scope {
                    Some(id) => vec![id],
                    None => self.libraries.iter().map(|l| l.id).collect(),
                };
                let mut tasks = Vec::new();
                for id in ids {
                    if let std::collections::hash_map::Entry::Vacant(e) = self.scans.entry(id) {
                        e.insert(scan_starting());
                        tasks.push(self.start_scan(id));
                    }
                }
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
                self.selected = Some(id);
                Task::none()
            }
            Message::Hover(id) => {
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
            Message::Escape if self.screen != Screen::Library => self.update(Message::Back),
            Message::Escape if self.filter_menu => {
                self.filter_menu = false;
                Task::none()
            }
            Message::Escape => {
                if self.search.query.text.is_empty() {
                    self.selected = None;
                    Task::none()
                } else {
                    self.update(Message::SearchChanged(String::new()))
                }
            }
            Message::SystemMode(mode) => {
                self.system_mode = match mode {
                    iced::theme::Mode::Dark => Mode::Dark,
                    iced::theme::Mode::Light | iced::theme::Mode::None => Mode::Light,
                };
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
            Message::ToggleFilterMenu => {
                self.filter_menu = !self.filter_menu;
                Task::none()
            }
            Message::ToggleChip(chip) => {
                filters::toggle(&mut self.chips, chip);
                let filter = filters::to_filter(&self.chips);
                match self.search.update(|q| q.filter = filter) {
                    Some(ticket) => self.run_search(ticket),
                    None => Task::none(),
                }
            }
            Message::OpenSelected => match (self.screen, self.selected) {
                (Screen::Library, Some(id)) => self.update(Message::OpenModel(id)),
                _ => Task::none(),
            },
            Message::OpenModel(id) => {
                self.selected = Some(id);
                self.screen = Screen::Model(id);
                self.model = Some(ModelView::Loading(id));
                self.load_model(id)
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
                    *part = index;
                }
                Task::none()
            }
            Message::OpenPart => match self.current_part().map(|p| p.rel_path.clone()) {
                Some(rel) if matches!(self.screen, Screen::Model(_)) => {
                    match self.model_path(&rel) {
                        Some(path) => self.update(Message::OpenFile(path)),
                        None => Task::none(),
                    }
                }
                _ => Task::none(),
            },
            Message::OpenFile(path) => launch(move || opener::open(&path)),
            Message::ShowInFolder(path) => launch(move || opener::reveal(&path)),
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
                Task::none()
            }
            Message::SettingsChanged(settings) => {
                let old = std::mem::replace(&mut self.settings, settings);
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
                Task::batch([search, self.save_settings()])
            }
            Message::SettingsSaved(Ok(())) => Task::none(),
            Message::SettingsSaved(Err(e)) => {
                tracing::warn!(error = %e, "could not save settings");
                self.error = Some(t("error-settings-save"));
                Task::none()
            }
        }
    }

    /// The part selected in the model view.
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
            *part = match mv {
                Move::Left => part.saturating_sub(1),
                Move::Right => (*part + 1).min(n - 1),
                Move::Home => 0,
                Move::End => n - 1,
                Move::Up | Move::Down => *part,
            };
        }
    }

    fn load_model(&self, id: ModelId) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.model(id).await.map_err(Arc::new) },
            move |r| Message::ModelLoaded(id, r),
        )
    }

    /// Writes the settings file off the UI thread.
    fn save_settings(&self) -> Task<Message> {
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
            EngineEvent::LibraryRemoved(_) | EngineEvent::LibraryRenamed(_) => {
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
            EngineEvent::ScanFinished { library, .. } => {
                self.scans.remove(&library);
                let ticket = self.search.refresh();
                let model = match &self.model {
                    Some(m) => self.load_model(m.id()),
                    None => Task::none(),
                };
                Task::batch([self.load_libraries(), self.run_search(ticket), model])
            }
            EngineEvent::ScanFailed { library, error } => {
                self.scans.remove(&library);
                if !matches!(*error, CoreError::Cancelled) {
                    self.show_error(&error);
                }
                Task::none()
            }
        }
    }

    fn set_scope(&mut self, scope: Option<LibraryId>) -> Task<Message> {
        self.viewport = self.viewport.map(|v| GridViewport { offset: 0.0, ..v });
        let scroll = operation::snap_to(grid_id(), RelativeOffset::START);
        match self.search.update(|q| q.scope = scope) {
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
        self.selected = Some(self.search.hits[index].id);
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

    fn show_error(&mut self, e: &CoreError) {
        tracing::warn!(error = %e, "engine error");
        self.error = Some(error_text(e));
    }

    fn load_libraries(&self) -> Task<Message> {
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
                        .search_in(q.scope, &q.text, &q.filter, q.sort, SEARCH_LIMIT)
                        .await?;
                    let total = if hits.len() < SEARCH_LIMIT as usize {
                        hits.len() as u64
                    } else {
                        engine.count(q.scope, &q.text, &q.filter).await?
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
        if let Some(engine) = &self.engine {
            subs.push(Subscription::run_with(
                EngineSub(engine.clone()),
                engine_events,
            ));
        }
        Subscription::batch(subs)
    }
}

/// Runs an OS hand-off (open, reveal) off the UI thread.
fn launch(f: impl FnOnce() -> Result<(), opener::OpenError> + Send + 'static) -> Task<Message> {
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
        _ => t("error-other"),
    }
}

/// Maps window and keyboard events to messages.
fn on_event(event: Event, status: event::Status, _window: window::Id) -> Option<Message> {
    match event {
        Event::Window(window::Event::Resized(size)) => Some(Message::WindowResized(size)),
        Event::Keyboard(keyboard::Event::KeyPressed { key, modifiers, .. }) => {
            let free = status == event::Status::Ignored;
            match key.as_ref() {
                keyboard::Key::Named(key::Named::Escape) => Some(Message::Escape),
                keyboard::Key::Character(",") if modifiers.command() => Some(Message::OpenSettings),
                keyboard::Key::Named(key::Named::Enter) if free => Some(Message::OpenSelected),
                keyboard::Key::Character("o") if free && !modifiers.command() => {
                    Some(Message::OpenPart)
                }
                keyboard::Key::Character("f") if modifiers.command() => Some(Message::FocusSearch),
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
            part_count: 1,
            missing: false,
            created_at: 0,
        }
    }

    /// Delivers `hits` as the results of a fresh run of the current query.
    fn deliver(app: &mut App, hits: Vec<ModelHit>) {
        let ticket = app.search.refresh();
        let total = hits.len() as u64;
        let _ = app.update(Message::Results(ticket, Ok((hits, total))));
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
        let _ = a.update(Message::SelectScope(Some(lib)));
        deliver(&mut a, vec![]);
        assert!(!a.search.loading);
        let _ = a.update(Message::Engine(EngineEvent::ScanFinished {
            library: lib,
            summary: ScanSummary::default(),
        }));
        assert!(a.search.loading);
        assert_eq!(
            (a.search.query.text.as_str(), a.search.query.scope),
            ("orc", Some(lib))
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
        let _ = a.update(Message::SelectScope(Some(lib)));
        assert_eq!(a.search.query.scope, Some(lib));
        assert!(a.search.loading);
    }

    #[test]
    fn removed_scope_falls_back_to_all() {
        let mut a = app();
        let _ = a.update(Message::SelectScope(Some(LibraryId::new())));
        deliver(&mut a, vec![]);
        let _ = a.update(Message::Libraries(Ok((vec![], HashMap::new()))));
        assert_eq!(a.search.query.scope, None);
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

    fn detail(id: ModelId, parts: &[&str]) -> ModelDetail {
        ModelDetail {
            id,
            library_id: LibraryId::new(),
            name: "Bin".into(),
            rel_path: "bins/bin".into(),
            kind: ModelKind::Folder,
            created_at: 0,
            parts: parts
                .iter()
                .map(|p| PartInfo {
                    id: printdex_core::PartId::new(),
                    rel_path: format!("bins/bin/{p}"),
                    format: printdex_core::PrintableFormat::Stl,
                    size: 10,
                })
                .collect(),
            assets: vec![],
        }
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
        let _ = a.update(Message::SelectScope(Some(LibraryId::new())));
        assert_eq!(a.chips, vec![Chip::MultiPart(true)]);
        assert_eq!(a.search.query.filter.multi_part, Some(true));
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
}
