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
    #[error("this folder contains links, which Printdex can't move between drives")]
    HasLinks(PathBuf),
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
    #[error("backup: {0}")]
    Backup(#[from] BackupError),
    #[error("sync: {0}")]
    Sync(#[from] SyncError),
}

/// Why a sync could not run or a file in a sync space was skipped.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SyncError {
    #[error("a file in the sync space is cut short")]
    Truncated,
    #[error("a file in the sync space is damaged")]
    Corrupt,
    #[error("the sync space was written by a newer Printdex (format {0})")]
    NewerFormat(u32),
    #[error("wrong sync passphrase")]
    WrongPassphrase,
    #[error("the sync passphrase is needed")]
    PassphraseNeeded,
    #[error("the sync space cannot be written")]
    ReadOnly,
    #[error("the sync space rejected the credentials")]
    Credentials,
    #[error("the sync space cannot be reached: {0}")]
    Unreachable(String),
    #[error("the sync space is full")]
    Full,
    #[error("sync space not found")]
    SpaceNotFound,
    #[error("this library is not linked to that sync space")]
    NotLinked,
    #[error("this library is already linked to that sync space")]
    AlreadyLinked,
    #[error("this device was seen too recently to be forgotten")]
    DeviceRecent,
    #[error("{0}")]
    Storage(String),
}

/// Why a backup or restore could not run, grouped by what the user can do
/// about it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BackupError {
    #[error("wrong passphrase")]
    WrongPassphrase,
    #[error("the target rejected the credentials")]
    Credentials,
    #[error("the target cannot be reached: {0}")]
    Unreachable(String),
    #[error("the target is full")]
    Full,
    #[error("the target is read-only")]
    ReadOnly,
    #[error("the target settings are not valid")]
    TargetInvalid,
    #[error("this kind of target is not available on this system")]
    Unsupported,
    #[error("no backup target is set up")]
    NotConfigured,
    #[error("the passphrase or target credentials are needed")]
    SecretsNeeded,
    #[error("there is no backup at the target")]
    NoRepository,
    #[error("snapshot not found")]
    SnapshotNotFound,
    #[error("the backup check found problems: {0}")]
    CheckFailed(String),
    #[error("{0}")]
    Repository(String),
}

/// Engine result type.
pub type Result<T, E = CoreError> = std::result::Result<T, E>;
