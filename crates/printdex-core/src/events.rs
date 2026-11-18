//! Engine events, published on a broadcast channel.

use std::path::PathBuf;
use std::sync::Arc;

use crate::db::Library;
use crate::error::CoreError;
use crate::ids::{LibraryId, ModelId};
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
        /// Set when the thumbnail was just rendered rather than found in
        /// the cache (the app plays the print-in then): where the model's
        /// top edge is, as a fraction of the image height from the top.
        print_in: Option<f32>,
    },
    /// What the user wrote about a model changed (tags, notes, recipe,
    /// print log…).
    ModelDataChanged(ModelId),
    /// The list of tags or collections changed (names or counts).
    OrganizeChanged,
    /// A missing model was forgotten.
    ModelForgotten(ModelId),
    /// A file action (rename, move, trash) finished or was undone.
    FileOpDone(crate::file_ops::OpSummary),
    /// A scan found sidecars written by another Printdex install.
    SidecarForeign {
        library: LibraryId,
    },
    /// A library's settings changed (sidecars, change detection).
    LibraryChanged(Library),
    /// The monitor reached a library's folder (first check, or it came back).
    LibraryOnline(LibraryId),
    /// The monitor could not reach a library's folder.
    LibraryOffline(LibraryId),
    /// No thumbnail for `target`.
    ThumbnailFailed {
        target: ThumbTarget,
        reason: ThumbFailure,
    },
}
