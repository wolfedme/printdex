//! Printdex engine: libraries, scanning, fingerprints, database and search.
//!
//! This crate has no UI dependencies. The desktop app and the `printdex` CLI
//! both drive it through the [`Engine`] handle.

pub mod cpu;
pub mod db;
pub mod error;
pub mod fingerprint;
pub mod grouping;
pub mod ids;
pub mod time;
pub mod walk;

pub use error::{CoreError, Result};
pub use ids::{AssetId, LibraryId, ModelId, PartId};

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
