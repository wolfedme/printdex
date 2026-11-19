//! Engine commands for what the user writes about a model: tags,
//! collections, notes, source, recipe, print log and photos.
//!
//! Every command writes the database, queues the model's sidecar and emits
//! [`EngineEvent::ModelDataChanged`] (plus [`EngineEvent::OrganizeChanged`]
//! when tag or collection lists change).

use std::path::Path;

use crate::db::Library;
use crate::engine::Engine;
use crate::error::{CoreError, Result};
use crate::events::EngineEvent;
use crate::ids::{AttemptId, CollectionId, LibraryId, ModelId, PhotoId};
use crate::model_data::{
    Attempt, CollectionInfo, Material, ModelData, NewAttempt, PhotoRef, Recipe, RecipeNote,
    TagCount,
};

impl Engine {
    fn changed(&self, id: ModelId) {
        self.sidecars.queue(id);
        let _ = self.events.send(EngineEvent::ModelDataChanged(id));
    }

    fn lists_changed(&self) {
        let _ = self.events.send(EngineEvent::OrganizeChanged);
    }

    /// Forgets a missing model and everything recorded about it (the
    /// Missing filter's Forget). Fails for a model that is present.
    pub async fn forget_model(&self, id: ModelId) -> Result<()> {
        self.db.forget_model(id).await?;
        let _ = self.events.send(EngineEvent::ModelForgotten(id));
        self.lists_changed();
        Ok(())
    }

    /// Everything the user wrote about model `id`.
    pub async fn model_data(&self, id: ModelId) -> Result<Option<ModelData>> {
        self.db.model_data(id).await
    }

    /// Sets the Markdown notes of model `id`.
    pub async fn set_notes(&self, id: ModelId, notes: &str) -> Result<()> {
        self.db.set_notes(id, notes).await?;
        self.changed(id);
        Ok(())
    }

    /// Sets or clears (`None`, blank) the source link of model `id`.
    pub async fn set_source(&self, id: ModelId, url: Option<&str>) -> Result<()> {
        self.db.set_source(id, url).await?;
        self.changed(id);
        Ok(())
    }

    /// Tags model `id` with `name`.
    pub async fn add_tag(&self, id: ModelId, name: &str) -> Result<()> {
        if self.db.add_tag(id, name).await? {
            self.changed(id);
            self.lists_changed();
        }
        Ok(())
    }

    /// Removes tag `name` from model `id`.
    pub async fn remove_tag(&self, id: ModelId, name: &str) -> Result<()> {
        if self.db.remove_tag(id, name).await? {
            self.changed(id);
            self.lists_changed();
        }
        Ok(())
    }

    /// Tags in use, most used first.
    pub async fn tags(&self) -> Result<Vec<TagCount>> {
        self.db.tags().await
    }

    /// All collections by name.
    pub async fn collections(&self) -> Result<Vec<CollectionInfo>> {
        self.db.collections().await
    }

    /// Creates an empty collection.
    pub async fn create_collection(&self, name: &str) -> Result<CollectionInfo> {
        let c = self.db.create_collection(name).await?;
        self.lists_changed();
        Ok(c)
    }

    /// Renames collection `id`.
    pub async fn rename_collection(&self, id: CollectionId, name: &str) -> Result<()> {
        self.db.rename_collection(id, name).await?;
        for m in self.db.collection_members(id).await? {
            self.changed(m);
        }
        self.lists_changed();
        Ok(())
    }

    /// Deletes collection `id`; its models stay.
    pub async fn delete_collection(&self, id: CollectionId) -> Result<()> {
        let members = self.db.collection_members(id).await?;
        self.db.delete_collection(id).await?;
        for m in members {
            self.changed(m);
        }
        self.lists_changed();
        Ok(())
    }

    /// Adds model `model` to the end of collection `id`.
    pub async fn add_to_collection(&self, id: CollectionId, model: ModelId) -> Result<()> {
        if self.db.add_to_collection(id, model).await? {
            self.changed(model);
            self.lists_changed();
        }
        Ok(())
    }

    /// Takes model `model` out of collection `id`.
    pub async fn remove_from_collection(&self, id: CollectionId, model: ModelId) -> Result<()> {
        if self.db.remove_from_collection(id, model).await? {
            self.changed(model);
            self.lists_changed();
        }
        Ok(())
    }

    /// Sets or removes the recipe of model `id`.
    pub async fn set_recipe(&self, id: ModelId, recipe: Option<&Recipe>) -> Result<()> {
        self.db.set_recipe(id, recipe).await?;
        self.changed(id);
        Ok(())
    }

    /// Sets (or clears) the recipe material of every model in `ids`,
    /// keeping the rest of each recipe ("Set material" on a selection).
    /// A recipe left with nothing in it is removed. Returns how many
    /// models changed.
    pub async fn set_material(&self, ids: &[ModelId], material: Option<Material>) -> Result<usize> {
        let mut changed = 0;
        for &id in ids {
            let old = self.db.model_data(id).await?.and_then(|d| d.recipe);
            let mut recipe = old.clone().unwrap_or_default();
            recipe.settings.material = material.clone();
            let new = (!recipe.is_empty()).then_some(recipe);
            if new != old {
                self.set_recipe(id, new.as_ref()).await?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// Records a print of model `id`.
    pub async fn log_attempt(&self, id: ModelId, attempt: &NewAttempt) -> Result<AttemptId> {
        let a = self.db.log_attempt(id, attempt).await?;
        self.changed(id);
        Ok(a)
    }

    /// Saves changes to an attempt.
    pub async fn update_attempt(&self, attempt: &Attempt) -> Result<()> {
        let model = self.db.update_attempt(attempt).await?;
        self.changed(model);
        Ok(())
    }

    /// Deletes an attempt and its photos.
    pub async fn delete_attempt(&self, id: AttemptId) -> Result<()> {
        let (model, photos) = self.db.delete_attempt(id).await?;
        let paths: Vec<_> = photos.into_iter().map(|p| p.path).collect();
        let _ = tokio::task::spawn_blocking(move || crate::photos::delete_files(&paths)).await;
        self.changed(model);
        Ok(())
    }

    /// Copies attempt `id`'s settings into its model's recipe ("Save as
    /// recipe"). Recipe notes and per-part overrides are kept.
    pub async fn recipe_from_attempt(&self, id: AttemptId) -> Result<()> {
        let (model, attempt) = self.db.attempt(id).await?;
        let mut recipe = self
            .db
            .model_data(model)
            .await?
            .and_then(|d| d.recipe)
            .unwrap_or_default();
        recipe.settings = attempt.settings;
        recipe.from_attempt = Some(id);
        self.set_recipe(model, Some(&recipe)).await
    }

    /// Adds `text` as a recipe note, scoped to the attempt's material ("Add
    /// as note" on a failed print).
    pub async fn note_from_attempt(&self, id: AttemptId, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            return Err(CoreError::TextTooLong);
        }
        let (model, attempt) = self.db.attempt(id).await?;
        let mut recipe = self
            .db
            .model_data(model)
            .await?
            .and_then(|d| d.recipe)
            .unwrap_or_default();
        recipe.notes.push(RecipeNote {
            material: attempt.settings.material,
            text: text.to_owned(),
        });
        self.set_recipe(model, Some(&recipe)).await
    }

    /// Copies an image into Printdex's data folder and attaches it to
    /// attempt `id`.
    pub async fn add_photo(&self, id: AttemptId, src: &Path) -> Result<PhotoRef> {
        let photo = PhotoId::new();
        let dir = self.db.photo_dir().to_path_buf();
        let src = src.to_path_buf();
        let name =
            tokio::task::spawn_blocking(move || crate::photos::store_photo(&dir, id, photo, &src))
                .await
                .map_err(|_| CoreError::PhotoInvalid)??;
        let path = self.db.photo_dir().join(id.to_string()).join(&name);
        match self.db.add_photo_row(id, photo, &name).await {
            Ok(model) => {
                self.changed(model);
                Ok(PhotoRef { id: photo, path })
            }
            Err(e) => {
                let p = vec![path];
                let _ = tokio::task::spawn_blocking(move || crate::photos::delete_files(&p)).await;
                Err(e)
            }
        }
    }

    /// Removes a photo.
    pub async fn remove_photo(&self, id: PhotoId) -> Result<()> {
        let (model, photo) = self.db.remove_photo_row(id).await?;
        let p = vec![photo.path];
        let _ = tokio::task::spawn_blocking(move || crate::photos::delete_files(&p)).await;
        self.changed(model);
        Ok(())
    }

    /// Printer names used so far, for autocomplete.
    pub async fn printers(&self) -> Result<Vec<String>> {
        self.db.printers().await
    }

    /// Turns sidecar files on or off for library `id`. Turning them on
    /// writes sidecars for every model with data, in the background.
    pub async fn set_sidecars(&self, id: LibraryId, enabled: bool) -> Result<Library> {
        self.db.set_sidecar_enabled(id, enabled).await?;
        if enabled {
            for m in self.db.models_with_data(id).await? {
                self.sidecars.queue(m);
            }
        }
        let lib = self.db.library(id).await?;
        let _ = self.events.send(EngineEvent::LibraryChanged(lib.clone()));
        Ok(lib)
    }

    /// Writes all queued sidecars now (tests, shutdown).
    pub async fn flush_sidecars(&self) {
        self.sidecars.flush().await;
    }
}
