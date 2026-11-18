//! The print side of the model view: recipe editor, print log, "Log a
//! print" form and attempt photos. The engine stores everything; this keeps
//! the form and what is expanded or being confirmed.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use iced::Task;
use iced::widget::{operation, text_editor};
use printdex_core::{
    Adhesion, AttemptId, AttemptResult, CoreError, Engine, ModelId, PhotoId, Supports,
};

use crate::app::{App, Message, Screen};
use crate::i18n::{Arg, t, t_args};
use crate::log_form::{Field, FormMode, LogForm};

/// Attempts shown before "Show N earlier attempts".
pub const SHOWN_ATTEMPTS: usize = 3;
/// How long the "Attempt 4 saved" toast stays.
const TOAST: Duration = Duration::from_millis(2600);
/// Printer suggestions under the field.
const PRINTER_SUGGESTIONS: usize = 4;

pub fn printer_input_id() -> iced::widget::Id {
    iced::widget::Id::new("form-printer")
}

pub fn note_input_id() -> iced::widget::Id {
    iced::widget::Id::new("attempt-note")
}

/// A text field of the form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Printer,
    Date,
    Material,
    Filament,
    Layer,
    Nozzle,
    Bed,
    Hotend,
    Infill,
    Walls,
    Duration,
    NewNote,
}

#[derive(Debug, Default)]
pub struct PrintsUi {
    pub form: Option<LogForm>,
    /// The form is saving.
    pub saving: bool,
    /// Every attempt is shown, not just the latest ones.
    pub show_all: bool,
    pub confirm_delete: Option<AttemptId>,
    /// "Add as note" on a failed attempt, with the text being edited.
    pub note_for: Option<(AttemptId, String)>,
    /// Printer names used before, for suggestions.
    pub printers: Vec<String>,
    /// "Attempt 4 saved".
    pub toast: Option<String>,
    toast_seq: u64,
    /// The attempt just saved, marked for a moment.
    pub fresh: Option<AttemptId>,
}

impl PrintsUi {
    /// Closes the form and inline prompts.
    pub fn reset(&mut self) {
        self.form = None;
        self.saving = false;
        self.confirm_delete = None;
        self.note_for = None;
        self.show_all = false;
    }

    /// Whether Esc has something to close.
    pub fn open(&self) -> bool {
        self.form.is_some() || self.confirm_delete.is_some() || self.note_for.is_some()
    }

    /// Known printers that start with (or contain) what was typed.
    pub fn printer_suggestions(&self, typed: &str) -> Vec<String> {
        let key = typed.trim().to_lowercase();
        let mut out: Vec<String> = self
            .printers
            .iter()
            .filter(|p| p.to_lowercase() != key)
            .filter(|p| key.is_empty() || p.to_lowercase().contains(&key))
            .cloned()
            .collect();
        out.sort_by_key(|p| !p.to_lowercase().starts_with(&key));
        out.truncate(PRINTER_SUGGESTIONS);
        out
    }
}

#[derive(Clone, Debug)]
pub enum Pr {
    OpenLog,
    OpenEdit(AttemptId),
    OpenRecipe,
    /// The recipe editor, showing one part's settings.
    OpenRecipePart(String),
    /// Recipe editor: show the whole model (`None`) or one part.
    Scope(Option<String>),
    /// Recipe editor: the shown part prints like the rest of the model.
    ClearPart,
    Close,
    Outcome(AttemptResult),
    Input(Input, String),
    Notes(text_editor::Action),
    Supports(Option<Supports>),
    Adhesion(Option<Adhesion>),
    ToggleRequired(&'static str),
    RemoveRecipeNote(usize),
    PickPhotos,
    PhotosPicked(Vec<PathBuf>),
    RemovePendingPhoto(usize),
    /// A file dropped on the window.
    FileDropped(PathBuf),
    Save,
    /// The attempt saved (or the recipe), with photo errors if any.
    Saved(Result<Option<(AttemptId, u32)>, Arc<CoreError>>),
    ShowAll(bool),
    AskDelete(AttemptId),
    ConfirmDelete(AttemptId),
    SaveAsRecipe(AttemptId),
    StartNote(AttemptId),
    NoteInput(String),
    CommitNote,
    RemovePhoto(PhotoId),
    OpenPhoto(PathBuf),
    Printers(Vec<String>),
    Done(Result<(), Arc<CoreError>>),
    HideToast(u64),
    Unmark(AttemptId),
}

impl From<Pr> for Message {
    fn from(p: Pr) -> Message {
        Message::Prints(p)
    }
}

/// Image files by extension (the engine checks the content).
fn is_image(p: &std::path::Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "webp"
        )
    })
}

fn run<F>(engine: Option<&Engine>, f: impl FnOnce(Engine) -> F + Send + 'static) -> Task<Message>
where
    F: std::future::Future<Output = Result<(), CoreError>> + Send + 'static,
{
    let Some(engine) = engine.cloned() else {
        return Task::none();
    };
    Task::perform(async move { f(engine).await.map_err(Arc::new) }, |r| {
        Message::Prints(Pr::Done(r))
    })
}

impl App {
    fn open_model_id(&self) -> Option<ModelId> {
        match self.screen {
            Screen::Model(id) => Some(id),
            _ => None,
        }
    }

    /// File names of the open model's parts, in the model view's order.
    fn part_names(&self, id: ModelId) -> Vec<String> {
        let Some(crate::app::ModelView::Loaded { detail, .. }) = &self.model else {
            return Vec::new();
        };
        if detail.id != id {
            return Vec::new();
        }
        let mut names: Vec<String> = Vec::new();
        for p in &detail.parts {
            let name = p.rel_path.rsplit('/').next().unwrap_or(&p.rel_path);
            if !names.iter().any(|n| n == name) {
                names.push(name.to_owned());
            }
        }
        names
    }

    /// Loads printer names for suggestions.
    pub fn load_printers(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move { engine.printers().await.unwrap_or_default() },
            |p| Message::Prints(Pr::Printers(p)),
        )
    }

    pub fn handle_prints(&mut self, msg: Pr) -> Task<Message> {
        let engine = self.engine.as_ref();
        match msg {
            Pr::OpenLog | Pr::OpenEdit(_) | Pr::OpenRecipe | Pr::OpenRecipePart(_) => {
                let Some(id) = self.open_model_id() else {
                    return Task::none();
                };
                let data = self.org.data_for(id);
                let recipe = data.and_then(|d| d.recipe.as_ref());
                let form = match msg {
                    Pr::OpenLog => {
                        let last = data
                            .and_then(|d| d.attempts.first())
                            .and_then(|a| a.settings.printer.as_deref());
                        LogForm::log(recipe, last)
                    }
                    Pr::OpenEdit(a) => {
                        let Some(a) = data.and_then(|d| d.attempts.iter().find(|x| x.id == a))
                        else {
                            return Task::none();
                        };
                        LogForm::edit(a)
                    }
                    Pr::OpenRecipePart(part) => {
                        let mut f = LogForm::recipe(recipe, &self.part_names(id));
                        // Nothing is typed yet, so this cannot fail.
                        let _ = f.select_scope(Some(part));
                        f
                    }
                    _ => LogForm::recipe(recipe, &self.part_names(id)),
                };
                self.org.cancel_edits();
                self.prints.confirm_delete = None;
                self.prints.note_for = None;
                self.prints.form = Some(form);
                self.motion.panel_opened(std::time::Instant::now());
                Task::batch([self.load_printers(), operation::focus(printer_input_id())])
            }
            Pr::Close => {
                self.prints.form = None;
                self.prints.saving = false;
                Task::none()
            }
            Pr::Scope(scope) => {
                if let Some(f) = &mut self.prints.form
                    && let Err(e) = f.select_scope(scope)
                {
                    f.error = Some(e);
                }
                Task::none()
            }
            Pr::ClearPart => {
                if let Some(f) = &mut self.prints.form {
                    f.clear_part();
                }
                Task::none()
            }
            Pr::Outcome(o) => {
                if let Some(f) = &mut self.prints.form {
                    f.outcome = o;
                }
                Task::none()
            }
            Pr::Input(field, value) => {
                if let Some(f) = &mut self.prints.form {
                    let slot = match field {
                        Input::Printer => &mut f.printer,
                        Input::Date => &mut f.date,
                        Input::Material => &mut f.material,
                        Input::Filament => &mut f.filament,
                        Input::Layer => &mut f.layer,
                        Input::Nozzle => &mut f.nozzle,
                        Input::Bed => &mut f.bed,
                        Input::Hotend => &mut f.hotend,
                        Input::Infill => &mut f.infill,
                        Input::Walls => &mut f.walls,
                        Input::Duration => &mut f.duration,
                        Input::NewNote => &mut f.new_note,
                    };
                    *slot = value;
                    f.error = None;
                }
                Task::none()
            }
            Pr::Notes(action) => {
                if let Some(f) = &mut self.prints.form {
                    f.notes.perform(action);
                }
                Task::none()
            }
            Pr::Supports(s) => {
                if let Some(f) = &mut self.prints.form {
                    f.supports = s;
                }
                Task::none()
            }
            Pr::Adhesion(a) => {
                if let Some(f) = &mut self.prints.form {
                    f.adhesion = a;
                }
                Task::none()
            }
            Pr::ToggleRequired(name) => {
                if let Some(f) = &mut self.prints.form
                    && !f.required.remove(name)
                {
                    f.required.insert(name.to_owned());
                }
                Task::none()
            }
            Pr::RemoveRecipeNote(i) => {
                if let Some(f) = &mut self.prints.form
                    && i < f.recipe_notes.len()
                {
                    f.recipe_notes.remove(i);
                }
                Task::none()
            }
            Pr::PickPhotos => Task::perform(
                async {
                    rfd::AsyncFileDialog::new()
                        .set_title(t("form-photo-pick"))
                        .add_filter(t("form-photo-filter"), &["png", "jpg", "jpeg", "webp"])
                        .pick_files()
                        .await
                        .map(|files| files.iter().map(|h| h.path().to_path_buf()).collect())
                        .unwrap_or_default()
                },
                |p| Message::Prints(Pr::PhotosPicked(p)),
            ),
            Pr::PhotosPicked(paths) => {
                if let Some(f) = &mut self.prints.form
                    && f.mode != FormMode::Recipe
                {
                    for p in paths {
                        if !f.photos.contains(&p) && f.photos.len() < printdex_core::db::MAX_PHOTOS
                        {
                            f.photos.push(p);
                        }
                    }
                }
                Task::none()
            }
            Pr::FileDropped(path) => {
                if is_image(&path) {
                    return self.update(Message::Prints(Pr::PhotosPicked(vec![path])));
                }
                Task::none()
            }
            Pr::RemovePendingPhoto(i) => {
                if let Some(f) = &mut self.prints.form
                    && i < f.photos.len()
                {
                    f.photos.remove(i);
                }
                Task::none()
            }
            Pr::Save => self.save_form(),
            Pr::Saved(Ok(saved)) => {
                self.prints.saving = false;
                self.prints.form = None;
                let Some((attempt, number)) = saved else {
                    return Task::none();
                };
                self.prints.fresh = Some(attempt);
                self.prints.toast_seq += 1;
                let seq = self.prints.toast_seq;
                self.prints.toast =
                    Some(t_args("log-saved", &[("n", Arg::str(number.to_string()))]));
                Task::batch([
                    Task::perform(tokio::time::sleep(TOAST), move |()| {
                        Message::Prints(Pr::HideToast(seq))
                    }),
                    Task::perform(tokio::time::sleep(Duration::from_millis(1400)), move |()| {
                        Message::Prints(Pr::Unmark(attempt))
                    }),
                ])
            }
            Pr::Saved(Err(e)) => {
                self.prints.saving = false;
                match (crate::log_form::error_message(&e), &mut self.prints.form) {
                    (Some(err), Some(f)) => f.error = Some(err),
                    _ => self.show_error(&e),
                }
                Task::none()
            }
            Pr::HideToast(seq) => {
                if seq == self.prints.toast_seq {
                    self.prints.toast = None;
                }
                Task::none()
            }
            Pr::Unmark(a) => {
                if self.prints.fresh == Some(a) {
                    self.prints.fresh = None;
                }
                Task::none()
            }
            Pr::ShowAll(all) => {
                self.prints.show_all = all;
                Task::none()
            }
            Pr::AskDelete(a) => {
                self.prints.confirm_delete = Some(a);
                Task::none()
            }
            Pr::ConfirmDelete(a) => {
                if self.prints.confirm_delete.take() != Some(a) {
                    return Task::none();
                }
                run(engine, move |e| async move { e.delete_attempt(a).await })
            }
            Pr::SaveAsRecipe(a) => run(
                engine,
                move |e| async move { e.recipe_from_attempt(a).await },
            ),
            Pr::StartNote(a) => {
                let text = self
                    .open_model_id()
                    .and_then(|id| self.org.data_for(id))
                    .and_then(|d| d.attempts.iter().find(|x| x.id == a))
                    .map(|x| x.notes.clone())
                    .unwrap_or_default();
                self.prints.note_for = Some((a, text));
                operation::focus(note_input_id())
            }
            Pr::NoteInput(s) => {
                if let Some((_, text)) = &mut self.prints.note_for {
                    *text = s;
                }
                Task::none()
            }
            Pr::CommitNote => {
                let Some((a, text)) = self.prints.note_for.take() else {
                    return Task::none();
                };
                if text.trim().is_empty() {
                    return Task::none();
                }
                run(engine, move |e| async move {
                    e.note_from_attempt(a, &text).await
                })
            }
            Pr::RemovePhoto(p) => run(engine, move |e| async move { e.remove_photo(p).await }),
            Pr::OpenPhoto(path) => crate::app::launch(move || opener::open(&path)),
            Pr::Printers(p) => {
                self.prints.printers = p;
                Task::none()
            }
            Pr::Done(Ok(())) => Task::none(),
            Pr::Done(Err(e)) => {
                self.show_error(&e);
                Task::none()
            }
        }
    }

    fn save_form(&mut self) -> Task<Message> {
        let (Some(id), Some(form)) = (self.open_model_id(), self.prints.form.as_mut()) else {
            return Task::none();
        };
        if self.prints.saving {
            return Task::none();
        }
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        let photos = form.photos.clone();
        let data = self
            .org
            .data
            .as_ref()
            .filter(|(m, _)| *m == id)
            .map(|(_, d)| d);
        let task = match form.mode {
            FormMode::Log => match form.new_attempt() {
                Ok(new) => {
                    let number = data.map_or(0, |d| d.attempts.len()) as u32 + 1;
                    Task::perform(
                        async move {
                            let a = engine.log_attempt(id, &new).await?;
                            for p in photos {
                                engine.add_photo(a, &p).await?;
                            }
                            Ok(Some((a, number)))
                        },
                        |r: Result<_, CoreError>| Message::Prints(Pr::Saved(r.map_err(Arc::new))),
                    )
                }
                Err(e) => {
                    form.error = Some(e);
                    return Task::none();
                }
            },
            FormMode::Edit(a) => {
                let Some(old) = data.and_then(|d| d.attempts.iter().find(|x| x.id == a)) else {
                    self.prints.form = None;
                    return Task::none();
                };
                match form.updated(old) {
                    Ok(updated) => Task::perform(
                        async move {
                            engine.update_attempt(&updated).await?;
                            for p in photos {
                                engine.add_photo(updated.id, &p).await?;
                            }
                            Ok(None)
                        },
                        |r: Result<_, CoreError>| Message::Prints(Pr::Saved(r.map_err(Arc::new))),
                    ),
                    Err(e) => {
                        form.error = Some(e);
                        return Task::none();
                    }
                }
            }
            FormMode::Recipe => match form.to_recipe() {
                Ok(recipe) => Task::perform(
                    async move {
                        engine.set_recipe(id, recipe.as_ref()).await?;
                        Ok(None)
                    },
                    |r: Result<_, CoreError>| Message::Prints(Pr::Saved(r.map_err(Arc::new))),
                ),
                Err(e) => {
                    form.error = Some(e);
                    return Task::none();
                }
            },
        };
        form.error = None;
        self.prints.saving = true;
        task
    }
}

/// Whether `field` has the form's error.
pub fn has_error(form: &LogForm, field: Field) -> bool {
    form.error.as_ref().is_some_and(|(f, _)| *f == field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printer_suggestions_prefer_prefix() {
        let ui = PrintsUi {
            printers: vec!["Prusa MK4".into(), "Bambu P1S".into(), "MK3S".into()],
            ..PrintsUi::default()
        };
        assert_eq!(ui.printer_suggestions("mk"), ["MK3S", "Prusa MK4"]);
        assert_eq!(ui.printer_suggestions("Prusa MK4"), Vec::<String>::new());
        assert_eq!(ui.printer_suggestions("").len(), 3);
    }

    #[test]
    fn only_images_are_taken_from_drops() {
        assert!(is_image(std::path::Path::new("/a/b.JPG")));
        assert!(!is_image(std::path::Path::new("/a/b.stl")));
    }
}
