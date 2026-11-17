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
    CoreError, Engine, EngineEvent, Library, LibraryId, ModelHit, ModelId, ScanPhase, ScanSummary,
};
use tokio_util::sync::CancellationToken;

use crate::grid::GridMetrics;
use crate::i18n::{Arg, t, t_args};
use crate::nav::{self, Move};
use crate::search::{SEARCH_LIMIT, SearchState, Ticket};
use crate::theme::{self, ColorTheme, Mode, Palette};

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

/// Where the engine keeps its data.
#[derive(Clone, Debug)]
pub struct Options {
    pub data_dir: PathBuf,
    /// Fixed light/dark; `None` follows the OS.
    pub mode: Option<Mode>,
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
    pub color_theme: ColorTheme,
    pub mode: Mode,
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
}

impl App {
    /// State before the engine is open.
    pub fn new(options: Options) -> App {
        let mode = options.mode.unwrap_or_default();
        App {
            options,
            engine: None,
            boot: Boot::Opening,
            libraries: Vec::new(),
            counts: HashMap::new(),
            scans: HashMap::new(),
            search: SearchState::default(),
            selected: None,
            hovered: None,
            error: None,
            color_theme: ColorTheme::default(),
            mode,
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

    pub fn palette(&self) -> Palette {
        Palette::new(self.color_theme, self.mode)
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
            Message::Nav(mv) => self.navigate(mv),
            Message::FocusSearch => Task::batch([
                operation::focus(search_id()),
                operation::select_all(search_id()),
            ]),
            Message::Escape => {
                if self.search.query.text.is_empty() {
                    self.selected = None;
                    Task::none()
                } else {
                    self.update(Message::SearchChanged(String::new()))
                }
            }
            Message::SystemMode(_) if self.options.mode.is_some() => Task::none(),
            Message::SystemMode(mode) => {
                self.mode = match mode {
                    iced::theme::Mode::Dark => Mode::Dark,
                    iced::theme::Mode::Light | iced::theme::Mode::None => Mode::Light,
                };
                Task::none()
            }
            Message::DismissError => {
                self.error = None;
                Task::none()
            }
        }
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
                Task::batch([self.load_libraries(), self.run_search(ticket)])
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
        let Some(index) = nav::step(self.selected_index(), mv, metrics.columns, len) else {
            return Task::none();
        };
        self.selected = Some(self.search.hits[index].id);
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
        App::new(Options {
            data_dir: PathBuf::from("/nonexistent"),
            mode: None,
        })
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
        assert_eq!(a.mode, Mode::Dark);
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Light));
        assert_eq!(a.mode, Mode::Light);
    }

    #[test]
    fn fixed_mode_ignores_system() {
        let mut a = App::new(Options {
            data_dir: PathBuf::from("/nonexistent"),
            mode: Some(Mode::Dark),
        });
        let _ = a.update(Message::SystemMode(iced::theme::Mode::Light));
        assert_eq!(a.mode, Mode::Dark);
    }
}
