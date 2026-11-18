//! Organizing state and logic: sidebar lists (collections, tags, smart
//! filters) and the open model's tags, collections, notes and source. The
//! engine holds the data; this keeps what is on screen and what is being
//! edited.

use std::sync::Arc;

use iced::Task;
use iced::widget::{markdown, operation, text_editor};
use printdex_core::{
    ChangeMode, CollectionId, CollectionInfo, CoreError, Engine, LibraryId, ModelData, ModelId,
    Scope, SmartCounts, TagCount,
};

use crate::app::{App, Message, Screen};
use crate::i18n::t;

/// Widget ids.
pub fn tag_input_id() -> iced::widget::Id {
    iced::widget::Id::new("tag-input")
}

pub fn source_input_id() -> iced::widget::Id {
    iced::widget::Id::new("source-input")
}

pub fn collection_input_id() -> iced::widget::Id {
    iced::widget::Id::new("collection-input")
}

/// Tags shown in the sidebar before "Show all tags".
pub const SIDEBAR_TAGS: usize = 12;
/// Tag suggestions under the tag field.
pub const SUGGESTIONS: usize = 6;

/// Sidebar lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lists {
    pub tags: Vec<TagCount>,
    pub collections: Vec<CollectionInfo>,
    pub smart: SmartCounts,
}

/// Where a new collection is being named.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewFor {
    /// The "+" next to Collections in the sidebar.
    Sidebar,
    /// The collection menu of the open model: add the model to it.
    Model(ModelId),
}

/// The collection shown as scope, being renamed or about to be deleted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollectionEdit {
    Renaming(CollectionId, String),
    ConfirmDelete(CollectionId),
}

#[derive(Debug, Default)]
pub struct OrgState {
    pub lists: Lists,
    /// The open model's data.
    pub data: Option<(ModelId, ModelData)>,
    /// Its notes, parsed for display.
    pub notes_md: Vec<markdown::Item>,
    /// The tag field, while open.
    pub tag_input: Option<String>,
    /// The notes editor, while editing.
    pub notes_edit: Option<text_editor::Content>,
    /// The source field, while editing.
    pub source_edit: Option<String>,
    /// The "add to collection" menu of the open model.
    pub collection_menu: bool,
    pub new_collection: Option<(NewFor, String)>,
    pub collection_edit: Option<CollectionEdit>,
    /// Show every tag in the sidebar, not just the most used.
    pub all_tags: bool,
    /// A neutral one-line notice (another install's sidecars).
    pub notice: Option<String>,
    /// The open missing model's Forget asks for confirmation.
    pub confirm_forget: bool,
}

impl OrgState {
    /// The open model's data, when it belongs to `id`.
    pub fn data_for(&self, id: ModelId) -> Option<&ModelData> {
        self.data.as_ref().filter(|(m, _)| *m == id).map(|(_, d)| d)
    }

    /// Whether some field of the model view is being edited.
    pub fn editing(&self) -> bool {
        self.tag_input.is_some()
            || self.notes_edit.is_some()
            || self.source_edit.is_some()
            || self.collection_menu
            || self.new_collection.is_some()
            || self.collection_edit.is_some()
            || self.confirm_forget
    }

    /// Closes every editor and menu.
    pub fn cancel_edits(&mut self) {
        self.tag_input = None;
        self.notes_edit = None;
        self.source_edit = None;
        self.collection_menu = false;
        self.new_collection = None;
        self.collection_edit = None;
        self.confirm_forget = false;
    }

    /// Existing tags that start with (or contain) what was typed, not yet on
    /// the model, best first.
    pub fn suggestions(&self, typed: &str, on_model: &[String]) -> Vec<String> {
        let key = typed.trim().to_lowercase();
        if key.is_empty() {
            return Vec::new();
        }
        let has = |n: &str| on_model.iter().any(|t| t.eq_ignore_ascii_case(n));
        let mut starts: Vec<&TagCount> = Vec::new();
        let mut contains: Vec<&TagCount> = Vec::new();
        for tag in &self.lists.tags {
            let lower = tag.name.to_lowercase();
            if has(&tag.name) || lower == key {
                continue;
            }
            if lower.starts_with(&key) {
                starts.push(tag);
            } else if lower.contains(&key) {
                contains.push(tag);
            }
        }
        starts
            .into_iter()
            .chain(contains)
            .take(SUGGESTIONS)
            .map(|t| t.name.clone())
            .collect()
    }
}

/// Organizing messages.
#[derive(Clone, Debug)]
pub enum Org {
    Lists(Result<Lists, Arc<CoreError>>),
    DataLoaded(ModelId, Result<Option<Box<ModelData>>, Arc<CoreError>>),
    /// An engine command finished.
    Done(Result<(), Arc<CoreError>>),
    StartTag,
    TagInput(String),
    SubmitTag,
    AddTag(String),
    RemoveTag(String),
    StartNotes,
    NotesEdit(text_editor::Action),
    SaveNotes,
    StartSource,
    SourceInput(String),
    SaveSource,
    OpenLink(String),
    ToggleCollectionMenu,
    AddToCollection(CollectionId),
    RemoveFromCollection(CollectionId),
    StartNewCollection(NewFor),
    NewCollectionInput(String),
    CommitNewCollection,
    Created(NewFor, Result<CollectionInfo, Arc<CoreError>>),
    StartRenameCollection(CollectionId),
    RenameCollectionInput(String),
    CommitRenameCollection,
    AskDeleteCollection(CollectionId),
    ConfirmDeleteCollection(CollectionId),
    /// Close editors and menus (Esc, Cancel buttons).
    Cancel,
    SetSidecars(LibraryId, bool),
    SetChangeMode(LibraryId, ChangeMode),
    /// Forget the open missing model: asks first.
    AskForget,
    ConfirmForget(ModelId),
    ShowAllTags(bool),
    DismissNotice,
}

impl From<Org> for Message {
    fn from(o: Org) -> Message {
        Message::Org(o)
    }
}

/// Runs an engine command and reports back with [`Org::Done`].
fn run<F>(engine: Option<&Engine>, f: impl FnOnce(Engine) -> F + Send + 'static) -> Task<Message>
where
    F: std::future::Future<Output = Result<(), CoreError>> + Send + 'static,
{
    let Some(engine) = engine.cloned() else {
        return Task::none();
    };
    Task::perform(async move { f(engine).await.map_err(Arc::new) }, |r| {
        Message::Org(Org::Done(r))
    })
}

impl App {
    /// The model shown in the model view.
    fn open_model(&self) -> Option<ModelId> {
        match self.screen {
            Screen::Model(id) => Some(id),
            _ => None,
        }
    }

    /// Reloads the sidebar lists.
    pub fn load_lists(&self) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                Ok::<_, CoreError>(Lists {
                    tags: engine.tags().await?,
                    collections: engine.collections().await?,
                    smart: engine.smart_counts().await?,
                })
            },
            |r| Message::Org(Org::Lists(r.map_err(Arc::new))),
        )
    }

    /// Loads model `id`'s data.
    pub fn load_data(&self, id: ModelId) -> Task<Message> {
        let Some(engine) = self.engine.clone() else {
            return Task::none();
        };
        Task::perform(
            async move {
                engine
                    .model_data(id)
                    .await
                    .map(|d| d.map(Box::new))
                    .map_err(Arc::new)
            },
            move |r| Message::Org(Org::DataLoaded(id, r)),
        )
    }

    pub fn handle_org(&mut self, msg: Org) -> Task<Message> {
        let engine = self.engine.as_ref();
        match msg {
            Org::Lists(Ok(lists)) => {
                self.org.lists = lists;
                // A deleted collection or vanished tag falls back to all models.
                let gone = match &self.search.query.scope {
                    Scope::Collection(id) => {
                        !self.org.lists.collections.iter().any(|c| c.id == *id)
                    }
                    _ => false,
                };
                if gone {
                    return self.set_scope(Scope::All);
                }
                Task::none()
            }
            Org::Lists(Err(e)) | Org::Done(Err(e)) => {
                self.show_error(&e);
                Task::none()
            }
            Org::Created(_, Err(e)) => {
                self.show_error(&e);
                Task::none()
            }
            Org::Done(Ok(())) => Task::none(),
            Org::DataLoaded(id, result) => {
                if self.open_model() != Some(id) {
                    return Task::none();
                }
                match result {
                    Ok(Some(data)) => {
                        self.org.notes_md = markdown::parse(&data.notes).collect();
                        let material = data.preview_material();
                        self.org.data = Some((id, *data));
                        return self.viewer.set_material(material).map(Message::Viewer);
                    }
                    Ok(None) => self.org.data = None,
                    Err(e) => self.show_error(&e),
                }
                Task::none()
            }
            Org::StartTag => {
                if self.open_model().is_none() {
                    return Task::none();
                }
                self.org.tag_input = Some(String::new());
                operation::focus(tag_input_id())
            }
            Org::TagInput(s) => {
                if let Some(t) = &mut self.org.tag_input {
                    *t = s;
                }
                Task::none()
            }
            Org::SubmitTag => {
                let typed = self.org.tag_input.clone().unwrap_or_default();
                if typed.trim().is_empty() {
                    self.org.tag_input = None;
                    return Task::none();
                }
                // Keep the field open for the next tag.
                self.org.tag_input = Some(String::new());
                self.update(Message::Org(Org::AddTag(typed)))
            }
            Org::AddTag(name) => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                if self.org.tag_input.is_some() {
                    self.org.tag_input = Some(String::new());
                }
                run(engine, move |e| async move { e.add_tag(id, &name).await })
            }
            Org::RemoveTag(name) => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                run(
                    engine,
                    move |e| async move { e.remove_tag(id, &name).await },
                )
            }
            Org::StartNotes => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                let notes = self
                    .org
                    .data_for(id)
                    .map(|d| d.notes.clone())
                    .unwrap_or_default();
                self.org.notes_edit = Some(text_editor::Content::with_text(&notes));
                operation::focus(crate::view::notes_editor_id())
            }
            Org::NotesEdit(action) => {
                if let Some(c) = &mut self.org.notes_edit {
                    c.perform(action);
                }
                Task::none()
            }
            Org::SaveNotes => {
                let (Some(id), Some(content)) = (self.open_model(), self.org.notes_edit.take())
                else {
                    return Task::none();
                };
                let mut notes = content.text();
                // The editor keeps a trailing newline.
                while notes.ends_with('\n') {
                    notes.pop();
                }
                // Show the new text right away.
                if let Some((_, d)) = self.org.data.as_mut().filter(|(m, _)| *m == id) {
                    d.notes = notes.clone();
                    self.org.notes_md = markdown::parse(&notes).collect();
                }
                run(
                    engine,
                    move |e| async move { e.set_notes(id, &notes).await },
                )
            }
            Org::StartSource => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                let current = self
                    .org
                    .data_for(id)
                    .and_then(|d| d.source.clone())
                    .unwrap_or_default();
                self.org.source_edit = Some(current);
                Task::batch([
                    operation::focus(source_input_id()),
                    operation::select_all(source_input_id()),
                ])
            }
            Org::SourceInput(s) => {
                if let Some(e) = &mut self.org.source_edit {
                    *e = s;
                }
                Task::none()
            }
            Org::SaveSource => {
                let (Some(id), Some(url)) = (self.open_model(), self.org.source_edit.clone())
                else {
                    return Task::none();
                };
                let url = url.trim().to_owned();
                // Bare "printables.com/…" gets https://.
                let url = if url.is_empty() || url.contains("://") {
                    url
                } else {
                    format!("https://{url}")
                };
                if !url.is_empty() && printdex_core::model_data::check_source(&url).is_err() {
                    self.error = Some(t("error-source-invalid"));
                    return Task::none();
                }
                self.org.source_edit = None;
                run(engine, move |e| async move {
                    e.set_source(id, Some(&url)).await
                })
            }
            Org::OpenLink(url) => {
                // Only web links leave the app.
                if printdex_core::model_data::check_source(&url).is_ok() {
                    crate::app::launch(move || opener::open_browser(&url))
                } else {
                    Task::none()
                }
            }
            Org::ToggleCollectionMenu => {
                self.org.collection_menu = !self.org.collection_menu;
                if !self.org.collection_menu {
                    self.org.new_collection = None;
                }
                Task::none()
            }
            Org::AddToCollection(c) => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                self.org.collection_menu = false;
                run(
                    engine,
                    move |e| async move { e.add_to_collection(c, id).await },
                )
            }
            Org::RemoveFromCollection(c) => {
                let Some(id) = self.open_model() else {
                    return Task::none();
                };
                run(engine, move |e| async move {
                    e.remove_from_collection(c, id).await
                })
            }
            Org::StartNewCollection(target) => {
                self.org.new_collection = Some((target, String::new()));
                operation::focus(collection_input_id())
            }
            Org::NewCollectionInput(s) => {
                if let Some((_, name)) = &mut self.org.new_collection {
                    *name = s;
                }
                Task::none()
            }
            Org::CommitNewCollection => {
                let Some((target, name)) = self.org.new_collection.clone() else {
                    return Task::none();
                };
                if name.trim().is_empty() {
                    self.org.new_collection = None;
                    return Task::none();
                }
                let Some(engine) = self.engine.clone() else {
                    return Task::none();
                };
                Task::perform(
                    async move { engine.create_collection(&name).await.map_err(Arc::new) },
                    move |r| Message::Org(Org::Created(target, r)),
                )
            }
            Org::Created(target, Ok(c)) => {
                self.org.new_collection = None;
                self.org.collection_menu = false;
                match target {
                    NewFor::Sidebar => Task::none(),
                    NewFor::Model(id) => run(engine, move |e| async move {
                        e.add_to_collection(c.id, id).await
                    }),
                }
            }
            Org::StartRenameCollection(id) => {
                let name = self
                    .org
                    .lists
                    .collections
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.name.clone())
                    .unwrap_or_default();
                self.org.collection_edit = Some(CollectionEdit::Renaming(id, name));
                Task::batch([
                    operation::focus(collection_input_id()),
                    operation::select_all(collection_input_id()),
                ])
            }
            Org::RenameCollectionInput(s) => {
                if let Some(CollectionEdit::Renaming(_, name)) = &mut self.org.collection_edit {
                    *name = s;
                }
                Task::none()
            }
            Org::CommitRenameCollection => {
                let Some(CollectionEdit::Renaming(id, name)) = self.org.collection_edit.take()
                else {
                    return Task::none();
                };
                if name.trim().is_empty() {
                    return Task::none();
                }
                run(engine, move |e| async move {
                    e.rename_collection(id, &name).await
                })
            }
            Org::AskDeleteCollection(id) => {
                self.org.collection_edit = Some(CollectionEdit::ConfirmDelete(id));
                Task::none()
            }
            Org::ConfirmDeleteCollection(id) => {
                if self.org.collection_edit.take() != Some(CollectionEdit::ConfirmDelete(id)) {
                    return Task::none();
                }
                run(
                    engine,
                    move |e| async move { e.delete_collection(id).await },
                )
            }
            Org::Cancel => {
                self.org.cancel_edits();
                Task::none()
            }
            Org::SetSidecars(lib, on) => run(engine, move |e| async move {
                e.set_sidecars(lib, on).await.map(|_| ())
            }),
            Org::SetChangeMode(lib, mode) => run(engine, move |e| async move {
                e.set_change_mode(lib, mode).await.map(|_| ())
            }),
            Org::AskForget => {
                self.org.cancel_edits();
                self.org.confirm_forget = true;
                Task::none()
            }
            Org::ConfirmForget(id) => {
                if !std::mem::take(&mut self.org.confirm_forget) {
                    return Task::none();
                }
                run(engine, move |e| async move { e.forget_model(id).await })
            }
            Org::ShowAllTags(all) => {
                self.org.all_tags = all;
                Task::none()
            }
            Org::DismissNotice => {
                self.org.notice = None;
                Task::none()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(name: &str, count: u64) -> TagCount {
        TagCount {
            name: name.into(),
            count,
        }
    }

    #[test]
    fn suggestions_prefer_prefix_and_skip_present() {
        let mut s = OrgState::default();
        s.lists.tags = vec![
            tc("functional", 9),
            tc("gridfinity", 5),
            tc("fun", 3),
            tc("defunct", 1),
        ];
        assert_eq!(
            s.suggestions("FUN", &["functional".into()]),
            Vec::<String>::new()
                .into_iter()
                .chain(["defunct".to_owned()])
                .collect::<Vec<_>>()
        );
        assert_eq!(s.suggestions("fu", &[]), ["functional", "fun", "defunct"]);
        assert!(s.suggestions("  ", &[]).is_empty());
    }
}
