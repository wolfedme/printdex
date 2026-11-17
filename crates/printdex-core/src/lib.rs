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
pub mod scan;
pub mod time;
pub mod walk;

pub use db::{Library, LibraryKind, ModelHit, Stats};
pub use engine::Engine;
pub use error::{CoreError, Result};
pub use events::{EngineEvent, ScanPhase};
pub use grouping::ModelKind;
pub use ids::{AssetId, LibraryId, ModelId, PartId};
pub use scan::ScanSummary;

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
