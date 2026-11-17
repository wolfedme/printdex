//! Mesh loading for Printdex: STL (binary and ASCII), 3MF and OBJ.
//!
//! Everything here is synchronous and CPU-bound; callers run it on a worker
//! pool. Files are read through plain buffered reads (never memory mapped)
//! and treated as hostile: declared counts are checked against the file
//! size, archive entries are read with a size cap, indices are bounds
//! checked, and non-finite coordinates are dropped.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

mod decimate;
mod obj;
mod stl;
mod threemf;

pub use decimate::{PREVIEW_TRIANGLES, decimate};
pub use threemf::thumbnail as threemf_thumbnail;

/// Buffer size for file reads: large enough that slow network shares see
/// few round trips.
pub(crate) const READ_BUFFER: usize = 1 << 20;

/// A printable file format this crate can read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Stl,
    ThreeMf,
    Obj,
}

/// A triangle soup: every three positions form one triangle, in
/// millimetres, Z up (the STL and 3MF convention).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
}

/// Axis-aligned bounding box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// Extent along each axis.
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    /// Middle of the box.
    pub fn center(&self) -> [f32; 3] {
        [
            (self.max[0] + self.min[0]) / 2.0,
            (self.max[1] + self.min[1]) / 2.0,
            (self.max[2] + self.min[2]) / 2.0,
        ]
    }
}

impl Mesh {
    /// Number of triangles.
    pub fn triangles(&self) -> usize {
        self.positions.len() / 3
    }

    /// Bounding box of all positions; `None` for an empty mesh.
    pub fn bounds(&self) -> Option<Bounds> {
        let first = *self.positions.first()?;
        let mut b = Bounds {
            min: first,
            max: first,
        };
        for p in &self.positions {
            for (i, v) in p.iter().enumerate() {
                b.min[i] = b.min[i].min(*v);
                b.max[i] = b.max[i].max(*v);
            }
        }
        Some(b)
    }

    /// Adds a triangle unless one of its coordinates is NaN or infinite.
    pub(crate) fn push_triangle(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
        if [a, b, c].iter().flatten().all(|v| v.is_finite()) {
            self.positions.extend_from_slice(&[a, b, c]);
        }
    }
}

/// Safety limits for loading untrusted files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Files with more triangles than this are refused.
    pub max_triangles: u64,
    /// Largest decompressed size read from one archive entry, in bytes.
    pub max_entry_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_triangles: 50_000_000,
            max_entry_bytes: 2 << 30,
        }
    }
}

/// Why a mesh could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum MeshError {
    #[error("could not read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a valid file: {0}")]
    Malformed(String),
    #[error("too large to preview ({triangles} triangles)")]
    TooLarge { triangles: u64 },
}

pub type Result<T> = std::result::Result<T, MeshError>;

pub(crate) fn malformed(msg: impl Into<String>) -> MeshError {
    MeshError::Malformed(msg.into())
}

/// Loads the mesh in the file at `path`. Blocking; run it on a worker.
pub fn load(path: &Path, format: Format, limits: Limits) -> Result<Mesh> {
    match format {
        Format::Stl => {
            let file = File::open(path)?;
            let size = file.metadata()?.len();
            stl::read(BufReader::with_capacity(READ_BUFFER, file), size, limits)
        }
        Format::ThreeMf => threemf::read(path, limits),
        Format::Obj => obj::read(
            BufReader::with_capacity(READ_BUFFER, File::open(path)?),
            limits,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_of_cube() {
        let mut m = Mesh::default();
        m.push_triangle([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 20.0, 0.0]);
        m.push_triangle([0.0, 0.0, 5.0], [10.0, 20.0, 5.0], [-1.0, 0.0, 5.0]);
        let b = m.bounds().unwrap();
        assert_eq!(b.min, [-1.0, 0.0, 0.0]);
        assert_eq!(b.max, [10.0, 20.0, 5.0]);
        assert_eq!(b.size(), [11.0, 20.0, 5.0]);
        assert_eq!(m.triangles(), 2);
    }

    #[test]
    fn empty_mesh_has_no_bounds() {
        assert!(Mesh::default().bounds().is_none());
    }

    #[test]
    fn drops_non_finite_vertices() {
        let mut m = Mesh::default();
        m.push_triangle([f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        m.push_triangle([0.0, 0.0, 0.0], [f32::INFINITY, 0.0, 0.0], [0.0, 1.0, 0.0]);
        m.push_triangle([0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]);
        assert_eq!(m.triangles(), 1);
    }
}
