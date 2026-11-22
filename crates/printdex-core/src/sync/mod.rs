//! Two-way sync of Printdex data between machines (sync design,
//! `docs/superpowers/specs/2026-11-18-printdex-sync-design.md`).
//!
//! Each install writes numbered change batches into its own folder of a
//! *sync space* and reads everyone else's. Model files are never touched.

pub mod crypto;
pub mod log;
pub mod store;

pub use log::{Batch, Hlc, Key, Record, Stamp};
pub use store::{FolderStore, MemStore, RemoteStore, SpaceStore, open_store};
