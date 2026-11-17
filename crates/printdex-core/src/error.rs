//! Engine error type.
//!
//! Variants carry codes and data, not sentences, so a UI can translate them.
//! The `Display` text is for logs and the CLI only.

use std::path::PathBuf;

use crate::ids::LibraryId;

/// Every error the engine can return.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("library not found: {0}")]
    LibraryNotFound(LibraryId),
    #[error("not a usable library folder: {}", .0.display())]
    LibraryPathInvalid(PathBuf),
    #[error("library overlaps an existing library at {}", existing.display())]
    LibraryOverlaps { existing: PathBuf },
    #[error("library folder is offline or unreadable: {}", .0.display())]
    LibraryOffline(PathBuf),
    #[error("library is already being scanned: {0}")]
    AlreadyScanning(LibraryId),
    #[error("cancelled")]
    Cancelled,
    #[error("i/o error at {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("database migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Engine result type.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
