//! Backup and restore (milestone 5): snapshots of Printdex's own data and,
//! per library, of model files, in a restic-format repository on a folder,
//! S3, WebDAV or SFTP target.

mod remote;
pub mod repo;

pub use repo::{
    BackupJob, KEEP, Keep, ProgressFn, Repo, RepoOptions, SnapshotFilter, SnapshotInfo, Target,
    TargetSecrets,
};
