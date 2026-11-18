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
    #[error("library name is empty")]
    LibraryNameInvalid,
    #[error("library folder is offline or unreadable: {}", .0.display())]
    LibraryOffline(PathBuf),
    #[error("library is already being scanned: {0}")]
    AlreadyScanning(LibraryId),
    #[error("tag is empty or too long")]
    TagInvalid,
    #[error("collection name is empty or too long")]
    CollectionNameInvalid,
    #[error("a collection with this name exists")]
    CollectionExists,
    #[error("collection not found")]
    CollectionNotFound,
    #[error("model not found")]
    ModelNotFound,
    #[error("attempt not found")]
    AttemptNotFound,
    #[error("source link must start with http:// or https://")]
    SourceInvalid,
    #[error("text is too long")]
    TextTooLong,
    #[error("{field} must be between {min} and {max}")]
    SettingOutOfRange {
        field: &'static str,
        min: f64,
        max: f64,
    },
    #[error("photo is not a supported image, too large, or one too many")]
    PhotoInvalid,
    #[error("cancelled")]
    Cancelled,
    #[error("not a usable file or folder name")]
    NameInvalid,
    #[error("something named {0} is already there")]
    NameTaken(String),
    #[error("the destination folder is offline or unreadable")]
    TargetOffline,
    #[error("the system trash cannot be used here")]
    TrashUnavailable,
    #[error("this action can no longer be undone")]
    UndoUnavailable,
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
