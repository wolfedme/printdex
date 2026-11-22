//! Two-way sync of Printdex data between machines (sync design,
//! `docs/superpowers/specs/2026-11-18-printdex-sync-design.md`).
//!
//! Each install writes numbered change batches into its own folder of a
//! *sync space* and reads everyone else's. Model files are never touched.

mod api;
pub mod crypto;
pub mod identity;
pub mod log;
pub mod merge;
pub(crate) mod run;
pub mod state;
pub mod store;

pub use api::{DEBOUNCE, FORGET_AFTER_MS, INTERVAL};
pub use api::{ParkedModel, RemoteLibrary, SpaceInfo, SyncAttribution};
pub use log::{Batch, Hlc, Key, Record, Stamp};
pub use run::{BATCH_RECORDS, SNAPSHOT_AFTER, SyncReport, library_space_dir};
pub use store::{FolderStore, MemStore, RemoteStore, SpaceStore, open_store};
