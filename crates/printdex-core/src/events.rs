//! Engine events, published on a broadcast channel.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db::Library;
use crate::error::CoreError;
use crate::ids::LibraryId;
use crate::scan::ScanSummary;
use crate::thumbs::{ThumbFailure, ThumbKind, ThumbTarget};

/// Stage of a running scan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanPhase {
    Walking,
    Fingerprinting,
    Writing,
}

/// Something that changed in the engine.
#[derive(Clone, Debug)]
pub enum EngineEvent {
    LibraryAdded(Library),
    LibraryRemoved(LibraryId),
    LibraryRenamed(Library),
    ScanStarted {
        library: LibraryId,
    },
    ScanProgress {
        library: LibraryId,
        phase: ScanPhase,
        done: u64,
        total: u64,
    },
    ScanFinished {
        library: LibraryId,
        summary: ScanSummary,
    },
    ScanFailed {
        library: LibraryId,
        error: Arc<CoreError>,
    },
    /// A thumbnail is in the cache at `path` (a PNG).
    ThumbnailReady {
        target: ThumbTarget,
        path: PathBuf,
        kind: ThumbKind,
    },
    /// No thumbnail for `target`.
    ThumbnailFailed {
        target: ThumbTarget,
        reason: ThumbFailure,
    },
}
