//! File actions in the app: multi-select, rename / move / trash dialogs,
//! the undo toast, and "Open with".

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::Task;
use iced::keyboard::Modifiers;
use iced::widget::{Id, operation};
use printdex_core::{
    CoreError, LibraryId, Material, ModelId, OpKind, OpState, OpSummary, PrintableFormat,
};

use crate::app::{App, Message, ModelView, Screen};
use crate::apps::{self, Apps};
use crate::i18n::{Arg, t, t_args};

/// How long the undo toast stays.
pub const TOAST: Duration = Duration::from_secs(10);

pub fn rename_field_id() -> Id {
    Id::new("rename-model")
}

/// The confirm dialog on screen.
#[derive(Clone, Debug, PartialEq)]
pub enum Dialog {
    Rename {
        id: ModelId,
        name: String,
        error: Option<String>,
    },
    Move {
        ids: Vec<ModelId>,
        names: Vec<String>,
        lib: LibraryId,
        folder: String,
        /// "Storage/Bins" in "Workshop".
        label: String,
        error: Option<String>,
    },
    Trash {
        ids: Vec<ModelId>,
        names: Vec<String>,
        error: Option<String>,
    },
}

#[derive(Default)]
pub struct FilesUi {
    /// Models picked with Ctrl/Shift-click, in click order. Empty means
    /// the selection is just `App::selected`.
    pub picked: Vec<ModelId>,
    /// Where a Shift-click range starts.
    pub anchor: Option<ModelId>,
    pub modifiers: Modifiers,
    pub dialog: Option<Dialog>,
    pub busy: bool,
    /// The last action, offered for undo, with the toast's sequence number.
    pub toast: Option<(OpSummary, u64)>,
    pub(crate) toast_seq: u64,
    /// A short note that replaces the toast (undone, or an error).
    pub note: Option<String>,
    pub apps: Apps,
    /// The Open button's menu.
    pub open_menu: bool,
    /// A short success note in the toast's place ("PETG set on 12 models").
    pub info: Option<String>,
    /// The selection bar's "Set material" menu.
    pub material_menu: bool,
}

#[derive(Clone, Debug)]
pub enum Fx {
    Modifiers(Modifiers),
    SelectAll,
    ClearSelection,
    AskRename,
    RenameInput(String),
    AskMove,
    MovePicked(Option<PathBuf>),
    AskTrash,
    Confirm,
    Cancel,
    Done(Result<OpSummary, Arc<CoreError>>),
    /// Undo the action in the toast.
    Undo(i64),
    /// Ctrl+Z: undo the newest action.
    UndoLast,
    HideToast(u64),
    ToggleOpenMenu,
    CloseOpenMenu,
    /// Open the current part (or all parts) with the system default app.
    OpenDefault,
    ChooseApp(PrintableFormat),
    AppChosen(PrintableFormat, Option<PathBuf>),
    ResetApp(PrintableFormat),
    ToggleMaterialMenu,
    /// Set (or with `None` clear) the recipe material of the selection.
    SetMaterial(Option<Material>),
    MaterialSet(Option<Material>, Result<usize, Arc<CoreError>>),
}

impl From<Fx> for Message {
    fn from(f: Fx) -> Message {
        Message::Files(f)
    }
}

/// Up to five names, then "and 3 more".
pub fn name_list(names: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut s = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        s.push_str(&t_args(
            "files-and-more",
            &[("count", Arg::count((names.len() - SHOWN) as u64))],
        ));
    }
    s
}

/// The toast text for a finished action.
pub fn toast_text(op: &OpSummary) -> String {
    let n = op.names.len() as u64;
    let first = op.names.first().cloned().unwrap_or_default();
    let args = [("count", Arg::count(n)), ("name", Arg::str(first))];
    match (op.kind, op.state) {
        (_, OpState::Undone) => t("files-undone"),
        (OpKind::Rename, _) => t_args("files-renamed", &args),
        (OpKind::Move, _) => t_args("files-moved", &args),
        (OpKind::Trash, _) if op.undoable => t_args("files-trashed", &args),
        (OpKind::Trash, _) => t_args("files-trashed-finder", &args),
    }
}

/// A file action error for a dialog, or `None` for one the app shows as a
/// general error.
pub fn error_text(e: &CoreError) -> String {
    match e {
        CoreError::NameInvalid => t("files-error-name"),
        CoreError::NameTaken(name) => {
            t_args("files-error-taken", &[("name", Arg::str(name.clone()))])
        }
        CoreError::HasLinks(_) => t("files-error-has-links"),
        CoreError::TargetOffline => t("files-error-offline"),
        CoreError::LibraryOffline(_) => t("files-error-source-offline"),
        CoreError::AlreadyScanning(_) => t("files-error-busy"),
        CoreError::TrashUnavailable => t("files-error-trash"),
        CoreError::UndoUnavailable => t("files-error-undo"),
        CoreError::ModelNotFound => t("files-error-gone"),
        _ => t("files-error-other"),
    }
}

impl App {
    /// The models a file action applies to: the picked set, else the open
    /// model, else the selected card.
    pub fn action_targets(&self) -> Vec<ModelId> {
        if let Screen::Model(id) = self.screen {
            return vec![id];
        }
        if !self.files.picked.is_empty() {
            return self.files.picked.clone();
        }
        self.selected.into_iter().collect()
    }

    /// Whether `id` is part of a multi-selection.
    pub fn picked(&self, id: ModelId) -> bool {
        self.files.picked.contains(&id)
    }

    fn name_of(&self, id: ModelId) -> String {
        if let Some(ModelView::Loaded { detail, .. }) = &self.model
            && detail.id == id
        {
            return detail.name.clone();
        }
        self.search
            .hits
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.name.clone())
            .unwrap_or_default()
    }

    fn lib_of(&self, id: ModelId) -> Option<LibraryId> {
        if let Some(ModelView::Loaded { detail, .. }) = &self.model
            && detail.id == id
        {
            return Some(detail.library_id);
        }
        self.search
            .hits
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.library_id)
    }

    /// A click on card `id`, with the current modifier keys: Ctrl/Cmd
    /// toggles it in the selection, Shift extends from the anchor, a plain
    /// click selects only it. Returns whether it was a plain click.
    pub fn click_select(&mut self, id: ModelId) -> bool {
        let m = self.files.modifiers;
        if m.command() {
            if self.files.picked.is_empty()
                && let Some(cur) = self.selected
                && cur != id
            {
                self.files.picked.push(cur);
            }
            if let Some(i) = self.files.picked.iter().position(|p| *p == id) {
                self.files.picked.remove(i);
            } else {
                self.files.picked.push(id);
            }
            self.files.anchor = Some(id);
            false
        } else if m.shift() {
            let anchor = self.files.anchor.or(self.selected).unwrap_or(id);
            let pos = |x: ModelId| self.search.hits.iter().position(|h| h.id == x);
            if let (Some(a), Some(b)) = (pos(anchor), pos(id)) {
                let (lo, hi) = (a.min(b), a.max(b));
                if let Some(span) = self.search.hits.get(lo..=hi) {
                    self.files.picked = span.iter().map(|h| h.id).collect();
                }
            }
            false
        } else {
            self.files.picked.clear();
            self.files.anchor = Some(id);
            true
        }
    }

    pub fn on_files(&mut self, msg: Fx) -> Task<Message> {
        let engine = self.engine.clone();
        match msg {
            Fx::Modifiers(m) => {
                self.files.modifiers = m;
                Task::none()
            }
            Fx::SelectAll => {
                if self.screen == Screen::Library {
                    self.files.picked = self.search.hits.iter().map(|h| h.id).collect();
                }
                Task::none()
            }
            Fx::ClearSelection => {
                self.files.picked.clear();
                Task::none()
            }
            Fx::AskRename => {
                let ids = self.action_targets();
                let [id] = ids.as_slice() else {
                    return Task::none();
                };
                self.files.dialog = Some(Dialog::Rename {
                    id: *id,
                    name: self.name_of(*id),
                    error: None,
                });
                Task::batch([
                    operation::focus(rename_field_id()),
                    operation::select_all(rename_field_id()),
                ])
            }
            Fx::RenameInput(s) => {
                if let Some(Dialog::Rename { name, error, .. }) = &mut self.files.dialog {
                    *name = s;
                    *error = None;
                }
                Task::none()
            }
            Fx::AskMove => {
                let ids = self.action_targets();
                let Some(first) = ids.first() else {
                    return Task::none();
                };
                let start = self
                    .lib_of(*first)
                    .and_then(|l| self.library(l))
                    .map(|l| l.path.clone());
                Task::perform(
                    async move {
                        let mut d = rfd::AsyncFileDialog::new().set_title(t("files-move-pick"));
                        if let Some(dir) = start {
                            d = d.set_directory(dir);
                        }
                        d.pick_folder().await.map(|h| h.path().to_path_buf())
                    },
                    |p| Message::Files(Fx::MovePicked(p)),
                )
            }
            Fx::MovePicked(None) => Task::none(),
            Fx::MovePicked(Some(path)) => {
                let ids = self.action_targets();
                let names = ids.iter().map(|id| self.name_of(*id)).collect();
                let path = dunce::canonicalize(&path).unwrap_or(path);
                let target = self.libraries.iter().find_map(|l| {
                    let rel = path.strip_prefix(&l.path).ok()?;
                    let parts: Option<Vec<&str>> =
                        rel.components().map(|c| c.as_os_str().to_str()).collect();
                    Some((l, parts?.join("/")))
                });
                self.files.dialog = Some(match target {
                    Some((lib, folder)) => Dialog::Move {
                        ids,
                        names,
                        lib: lib.id,
                        label: if folder.is_empty() {
                            lib.display_name.clone()
                        } else {
                            t_args(
                                "files-move-where",
                                &[
                                    ("folder", Arg::str(folder.clone())),
                                    ("library", Arg::str(lib.display_name.clone())),
                                ],
                            )
                        },
                        folder,
                        error: None,
                    },
                    None => Dialog::Move {
                        ids,
                        names,
                        lib: LibraryId(printdex_core::ids::Uuid::nil()),
                        folder: String::new(),
                        label: path.display().to_string(),
                        error: Some(t("files-error-outside")),
                    },
                });
                Task::none()
            }
            Fx::AskTrash => {
                let ids = self.action_targets();
                if ids.is_empty() {
                    return Task::none();
                }
                let names = ids.iter().map(|id| self.name_of(*id)).collect();
                self.files.dialog = Some(Dialog::Trash {
                    ids,
                    names,
                    error: None,
                });
                Task::none()
            }
            Fx::Cancel => {
                self.files.dialog = None;
                self.files.open_menu = false;
                self.files.material_menu = false;
                Task::none()
            }
            Fx::Confirm => {
                let (Some(engine), Some(dialog)) = (engine, self.files.dialog.clone()) else {
                    return Task::none();
                };
                if self.files.busy {
                    return Task::none();
                }
                let job: std::pin::Pin<Box<dyn Future<Output = _> + Send>> = match dialog {
                    Dialog::Rename { id, name, .. } => {
                        Box::pin(async move { engine.rename_model(id, &name).await })
                    }
                    Dialog::Move {
                        ids,
                        lib,
                        folder,
                        error: None,
                        ..
                    } => Box::pin(async move { engine.move_models(&ids, lib, &folder).await }),
                    Dialog::Move { .. } => return Task::none(),
                    Dialog::Trash { ids, .. } => {
                        Box::pin(async move { engine.trash_models(&ids).await })
                    }
                };
                self.files.busy = true;
                Task::perform(async move { job.await.map_err(Arc::new) }, |r| {
                    Message::Files(Fx::Done(r))
                })
            }
            Fx::Done(Ok(_)) => {
                // The engine's FileOpDone brings the toast.
                self.files.busy = false;
                self.files.dialog = None;
                self.files.picked.clear();
                Task::none()
            }
            Fx::Done(Err(e)) => {
                self.files.busy = false;
                let text = error_text(&e);
                match &mut self.files.dialog {
                    Some(
                        Dialog::Rename { error, .. }
                        | Dialog::Move { error, .. }
                        | Dialog::Trash { error, .. },
                    ) => *error = Some(text),
                    None => self.files.note = Some(text),
                }
                Task::none()
            }
            Fx::Undo(op) => {
                self.files.toast = None;
                let Some(engine) = engine else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.undo(op).await.map_err(Arc::new) },
                    |r| Message::Files(Fx::Done(r)),
                )
            }
            Fx::UndoLast => {
                if let Some((op, _)) = &self.files.toast
                    && op.undoable
                {
                    let id = op.id;
                    return self.on_files(Fx::Undo(id));
                }
                let Some(engine) = engine else {
                    return Task::none();
                };
                Task::perform(
                    async move {
                        match engine.last_op().await {
                            Ok(Some(op)) => engine.undo(op.id).await.map_err(Arc::new),
                            Ok(None) => Err(Arc::new(CoreError::UndoUnavailable)),
                            Err(e) => Err(Arc::new(e)),
                        }
                    },
                    |r| Message::Files(Fx::Done(r)),
                )
            }
            Fx::HideToast(seq) => {
                if self.files.toast.as_ref().is_some_and(|(_, s)| *s == seq) {
                    self.files.toast = None;
                }
                if self.files.toast.is_none() {
                    self.files.note = None;
                    self.files.info = None;
                }
                Task::none()
            }
            Fx::ToggleMaterialMenu => {
                self.files.material_menu = !self.files.material_menu;
                Task::none()
            }
            Fx::SetMaterial(material) => {
                self.files.material_menu = false;
                let ids = self.action_targets();
                let Some(engine) = engine.filter(|_| !ids.is_empty()) else {
                    return Task::none();
                };
                let m = material.clone();
                Task::perform(
                    async move { engine.set_material(&ids, m).await.map_err(Arc::new) },
                    move |r| Message::Files(Fx::MaterialSet(material.clone(), r)),
                )
            }
            Fx::MaterialSet(material, result) => {
                let (text, ok) = match result {
                    Ok(n) => {
                        let n = n as u64;
                        let text = match material {
                            Some(m) => t_args(
                                "files-material-set",
                                &[
                                    ("count", Arg::count(n)),
                                    ("material", Arg::str(m.label().to_owned())),
                                ],
                            ),
                            None => t_args("files-material-cleared", &[("count", Arg::count(n))]),
                        };
                        (text, true)
                    }
                    Err(e) => (error_text(&e), false),
                };
                // A short note in the toast's place; it goes like a toast.
                self.files.toast = None;
                if ok {
                    self.files.note = None;
                    self.files.info = Some(text);
                } else {
                    self.files.info = None;
                    self.files.note = Some(text);
                }
                self.files.toast_seq += 1;
                let seq = self.files.toast_seq;
                Task::perform(tokio::time::sleep(TOAST), move |()| {
                    Message::Files(Fx::HideToast(seq))
                })
            }
            Fx::ToggleOpenMenu => {
                self.files.open_menu = !self.files.open_menu;
                Task::none()
            }
            Fx::CloseOpenMenu => {
                self.files.open_menu = false;
                Task::none()
            }
            Fx::OpenDefault => {
                self.files.open_menu = false;
                match self.current_part_path() {
                    Some(path) => self.update(Message::OpenFile(path)),
                    None => Task::none(),
                }
            }
            Fx::ChooseApp(format) => {
                self.files.open_menu = false;
                Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .set_title(t("apps-choose"))
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    move |p| Message::Files(Fx::AppChosen(format, p)),
                )
            }
            Fx::AppChosen(_, None) => Task::none(),
            Fx::AppChosen(format, Some(app)) => {
                self.files.apps.set(format, Some(app));
                self.save_apps()
            }
            Fx::ResetApp(format) => {
                self.files.apps.set(format, None);
                self.save_apps()
            }
        }
    }

    /// A file action finished (or was undone) in the engine.
    pub fn file_op_done(&mut self, op: OpSummary) -> Task<Message> {
        self.files.toast_seq += 1;
        let seq = self.files.toast_seq;
        self.files.note = None;
        self.files.info = None;
        self.files.toast = Some((op, seq));
        Task::perform(tokio::time::sleep(TOAST), move |()| {
            Message::Files(Fx::HideToast(seq))
        })
    }

    fn save_apps(&self) -> Task<Message> {
        if self.apps_kept {
            tracing::warn!("apps.toml is kept (unreadable, or restored); not saving apps");
            return Task::none();
        }
        let Some(dir) = self.options.config_dir.clone() else {
            return Task::none();
        };
        let apps = self.files.apps.clone();
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || apps.save(&dir))
                    .await
                    .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                    .map_err(Arc::new)
            },
            Message::SettingsSaved,
        )
    }

    /// The app chosen for the current part's format, if any.
    pub fn open_with(&self) -> Option<&std::path::Path> {
        self.current_part()
            .and_then(|p| self.files.apps.get(p.format))
    }

    /// Opens the current part: in the chosen app, else the system default.
    pub fn open_part(&mut self) -> Task<Message> {
        let Some(path) = self.current_part_path() else {
            return Task::none();
        };
        let Some(app) = self.open_with().map(std::path::Path::to_path_buf) else {
            return self.update(Message::OpenFile(path));
        };
        Task::perform(
            async move {
                tokio::task::spawn_blocking(move || {
                    apps::open_command(&app, &[path]).spawn().map(|_| ())
                })
                .await
                .unwrap_or_else(|e| Err(std::io::Error::other(e)))
                .map_err(Arc::new)
            },
            Message::Launched,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_across_drives_have_their_own_text() {
        let e = CoreError::HasLinks(std::path::PathBuf::from("Hook"));
        assert_eq!(
            error_text(&e),
            "This folder contains links, which Printdex can't move between drives."
        );
    }
}
