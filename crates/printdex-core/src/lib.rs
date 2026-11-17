//! Printdex engine: libraries, scanning, fingerprints, database and search.
//!
//! This crate has no UI dependencies. The desktop app and the `printdex` CLI
//! both drive it through the [`Engine`] handle.

pub mod cpu;
pub mod db;
pub mod engine;
pub mod error;
pub mod events;
pub mod fingerprint;
pub mod grouping;
pub mod ids;
pub mod model_data;
pub mod photos;
pub mod scan;
pub mod sidecar;
pub mod thumbs;
pub mod time;
mod user_data;
pub mod walk;

pub use db::{
    AssetInfo, Library, LibraryKind, ModelDetail, ModelHit, PartInfo, RecipeFacts, Scope,
    SearchFilter, Smart, SmartCounts, SortOrder, Stats, format_number,
};
pub use engine::{Engine, LibraryStatus};
pub use error::{CoreError, Result};
pub use events::{EngineEvent, ScanPhase};
pub use grouping::ModelKind;
pub use ids::{AssetId, AttemptId, CollectionId, LibraryId, ModelId, PartId, PhotoId, TagId};
pub use model_data::{
    Adhesion, Attempt, AttemptResult, CollectionInfo, LastPrint, Material, ModelData, NewAttempt,
    PartOverride, PhotoRef, PrintSettings, Recipe, RecipeNote, Supports, TagCount,
};
pub use scan::ScanSummary;
pub use thumbs::{
    DEFAULT_CACHE_LIMIT, Detail, Preview, THUMB_HEIGHT, THUMB_WIDTH, ThumbFailure, ThumbKind,
    ThumbTarget, ThumbnailService,
};
pub use walk::PrintableFormat;

/// Version of the engine crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_set() {
        assert!(!VERSION.is_empty());
    }
}
