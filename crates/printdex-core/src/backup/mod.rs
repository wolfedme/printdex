//! Backup and restore (milestone 5): snapshots of Printdex's own data and,
//! per library, of model files, in a restic-format repository on a folder,
//! S3, WebDAV or SFTP target.

pub mod config;
mod data;
pub(crate) mod remote;
pub mod repo;
pub mod restore;
pub(crate) mod run;
pub mod scheduler;
pub mod secrets;

pub use crate::db::backup::RunRow as BackupRun;
pub use config::{
    BackupConfig, ModelSchedule, Schedule, TargetCheck, TargetConfig, default_models_on,
};
pub use repo::{
    BackupJob, Globs, KEEP, Keep, ProgressFn, Repo, RepoOptions, SnapshotFilter, SnapshotInfo,
    Target, TargetSecrets,
};
pub use restore::{FileChange, FileDiff, ModelRestoreDest, RELINK, RestoredData, SIDECARS_FROM_DB};
pub use run::{BackupScope, DATA_TAG, RunSummary, models_tag};
pub use scheduler::{BackupScheduler, SchedulerConfig};

use crate::ids::LibraryId;

/// What a backup job backs up or maintains.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BackupKind {
    /// Printdex's own data (database, photos, settings).
    Data,
    /// One library's model files.
    Models(LibraryId),
    /// Removing data no snapshot needs.
    Prune,
    /// Reading part of the repository back.
    Check,
}

/// Something about backups the user should look at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attention {
    /// No data backup for 3 days.
    DataStale,
    /// No model backup of a library for 14 days.
    ModelsStale(LibraryId),
    /// A library with model backup on was offline for 14 days.
    ConnectLibrary(LibraryId),
    TargetUnreachable,
    TargetFull,
    /// The target rejected the stored credentials, or none are stored.
    Credentials,
    /// The repository passphrase is needed (not stored, or wrong).
    PassphraseNeeded,
    CheckFailed,
}
