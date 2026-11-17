//! Vertex-clustering decimation: makes a large mesh small enough to orbit
//! smoothly in the 3D viewer.
//!
//! Space is cut into a grid of equal cubes; every vertex in a cube moves
//! to the average position of that cube's vertices, and triangles that
//! collapse (two corners in one cube) or repeat disappear. The grid is
//! refined or coarsened until the result fits the target. It is fast and
//! robust on any triangle soup (no topology needed), at the cost of some
//! detail on fine features, which a preview can spare.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

use crate::Mesh;

/// Default triangle budget for interactive previews.
pub const PREVIEW_TRIANGLES: usize = 400_000;

/// Finest grid tried, cells along the longest side. Keys pack three
/// 21-bit cell coordinates into one `u64`.
const MAX_CELLS: u32 = (1 << 20) - 1;
/// Grid passes before giving up on refining (the last one always fits).
const MAX_PASSES: usize = 8;

/// Returns `mesh` with at most `target` triangles. A mesh already within
/// the target comes back unchanged.
pub fn decimate(mesh: Mesh, target: usize) -> Mesh {
    let tris = mesh.triangles();
    if tris <= target {
        return mesh;
    }
    let Some(bounds) = mesh.bounds() else {
        return mesh;
    };
    if target == 0 {
        return Mesh::default();
    }
    let size = bounds.size();
    let longest = size[0].max(size[1]).max(size[2]);
    if longest <= 0.0 || !longest.is_finite() {
        return Mesh::default();
    }

    let welded = Welded::new(&mesh);
    drop(mesh);

    // Surface triangles grow with the square of the grid resolution, so
    // start from a guess and correct by the square root of the miss.
    // Counting a pass skips duplicate removal, so its count is an upper
    // bound of the final one.
    let mut cells = ((target as f64 / 2.0).sqrt() as u32).clamp(1, MAX_CELLS);
    let mut best: Option<(Vec<u32>, usize)> = None;
    for pass in 0..MAX_PASSES {
        let (clusters, n) = welded.cluster(bounds.min, longest, cells);
        if n <= target {
            let ratio = n as f64 / target as f64;
            if best.as_ref().is_none_or(|b| n > b.1) {
                best = Some((clusters, n));
            }
            // Close enough to the budget, or can't refine further.
            if ratio > 0.8 || cells >= MAX_CELLS || pass + 1 == MAX_PASSES {
                break;
            }
            let grow = (1.0 / ratio.max(1e-6)).sqrt() * 0.95;
            cells = ((f64::from(cells) * grow) as u32).clamp(cells + 1, MAX_CELLS);
        } else {
            if best.is_some() {
                // A coarser grid fitted; refining overshot.
                break;
            }
            if cells == 1 {
                return Mesh::default();
            }
            let shrink = (target as f64 / n as f64).sqrt() * 0.95;
            let next = ((f64::from(cells) * shrink) as u32).clamp(1, cells - 1);
            // The last pass must fit: fall back to halving.
            cells = if pass + 2 == MAX_PASSES { 1 } else { next };
        }
    }
    match best {
        Some((clusters, _)) => welded.build(&clusters),
        None => Mesh::default(),
    }
}

/// The mesh with identical corners merged: unique positions and
/// triangles as index triples.
struct Welded {
    verts: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
}

impl Welded {
    fn new(mesh: &Mesh) -> Welded {
        let mut index: HashMap<[u32; 3], u32, Fx> =
            HashMap::with_capacity_and_hasher(mesh.positions.len() / 4, Fx::default());
        let mut verts = Vec::new();
        let mut id = |p: &[f32; 3]| {
            *index.entry(p.map(f32::to_bits)).or_insert_with(|| {
                verts.push(*p);
                (verts.len() - 1) as u32
            })
        };
        let tris = mesh
            .positions
            .as_chunks::<3>()
            .0
            .iter()
            .map(|t| [id(&t[0]), id(&t[1]), id(&t[2])])
            .collect();
        Welded { verts, tris }
    }

    /// Cluster per vertex for a grid of `cells` cubes along the longest
    /// side, and how many triangles survive (before duplicate removal).
    fn cluster(&self, min: [f32; 3], longest: f32, cells: u32) -> (Vec<u32>, usize) {
        let inv = cells as f32 / longest;
        let max = cells - 1;
        let mut index: HashMap<u64, u32, Fx> = HashMap::default();
        let clusters: Vec<u32> = self
            .verts
            .iter()
            .map(|p| {
                let c = |i: usize| u64::from((((p[i] - min[i]) * inv) as u32).min(max));
                let key = c(0) | (c(1) << 21) | (c(2) << 42);
                let next = index.len() as u32;
                *index.entry(key).or_insert(next)
            })
            .collect();
        let n = self
            .tris
            .iter()
            .filter(|t| {
                let [a, b, c] = t.map(|v| clusters[v as usize]);
                a != b && b != c && a != c
            })
            .count();
        (clusters, n)
    }

    /// The decimated soup: each cluster at the average of its vertices,
    /// collapsed and repeated triangles dropped.
    fn build(&self, clusters: &[u32]) -> Mesh {
        let count = clusters.iter().max().map_or(0, |m| *m as usize + 1);
        let mut sums = vec![([0.0f64; 3], 0u32); count];
        for (p, c) in self.verts.iter().zip(clusters) {
            let s = &mut sums[*c as usize];
            for (a, v) in s.0.iter_mut().zip(p) {
                *a += f64::from(*v);
            }
            s.1 += 1;
        }
        let centers: Vec<[f32; 3]> = sums
            .iter()
            .map(|(s, n)| {
                let n = f64::from((*n).max(1));
                [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32]
            })
            .collect();
        let mut seen: HashSet<[u32; 3], Fx> = HashSet::default();
        let mut out = Mesh::default();
        for t in &self.tris {
            let [a, b, c] = t.map(|v| clusters[v as usize]);
            if a == b || b == c || a == c {
                continue;
            }
            let mut key = [a, b, c];
            key.sort_unstable();
            if seen.insert(key) {
                out.positions.extend_from_slice(&[
                    centers[a as usize],
                    centers[b as usize],
                    centers[c as usize],
                ]);
            }
        }
        out
    }
}

/// A small multiplicative hasher: the keys are already well mixed grid
/// coordinates, and SipHash would dominate the pass.
#[derive(Default)]
struct FxHasher(u64);

type Fx = BuildHasherDefault<FxHasher>;

impl Hasher for FxHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(u64::from(*b));
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(u64::from(n));
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = (self.0.rotate_left(5) ^ n).wrapping_mul(0x51_7c_c1_b7_27_22_0a_95);
    }

    fn write_usize(&mut self, n: usize) {
        self.write_u64(n as u64);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A UV sphere of radius `r` with `n` rings and `2n` segments.
    fn sphere(r: f32, n: usize) -> Mesh {
        let mut m = Mesh::default();
        let p = |i: usize, j: usize| {
            let th = std::f32::consts::PI * i as f32 / n as f32;
            let ph = std::f32::consts::TAU * j as f32 / (2 * n) as f32;
            [
                r * th.sin() * ph.cos(),
                r * th.sin() * ph.sin(),
                r * th.cos() + r,
            ]
        };
        for i in 0..n {
            for j in 0..2 * n {
                let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
                m.push_triangle(a, b, c);
                m.push_triangle(a, c, d);
            }
        }
        m
    }

    #[test]
    fn decimate_hits_target() {
        let m = sphere(20.0, 400); // 640k triangles
        assert!(m.triangles() > 600_000);
        for target in [100_000, 20_000, 1_000] {
            let d = decimate(m.clone(), target);
            assert!(d.triangles() <= target, "{} > {target}", d.triangles());
            assert!(
                d.triangles() >= target / 3,
                "{} far below {target}",
                d.triangles()
            );
        }
    }

    #[test]
    fn decimate_keeps_bounds() {
        let m = sphere(20.0, 300);
        let before = m.bounds().unwrap();
        let d = decimate(m, 10_000);
        let after = d.bounds().unwrap();
        for i in 0..3 {
            let tol = 0.05 * before.size()[i].max(1.0);
            assert!((before.min[i] - after.min[i]).abs() < tol, "min {i}");
            assert!((before.max[i] - after.max[i]).abs() < tol, "max {i}");
        }
    }

    #[test]
    fn small_mesh_is_unchanged() {
        let m = sphere(5.0, 10);
        assert_eq!(decimate(m.clone(), 1_000), m);
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        assert_eq!(decimate(Mesh::default(), 10).triangles(), 0);
        // Everything at one point.
        let mut m = Mesh::default();
        for _ in 0..100 {
            m.push_triangle([1.0; 3], [1.0; 3], [1.0; 3]);
        }
        assert_eq!(decimate(m, 10).triangles(), 0);
        assert_eq!(decimate(sphere(1.0, 20), 0).triangles(), 0);
        // Huge coordinates.
        let mut m = sphere(1.0, 40);
        m.push_triangle([-1e30, 0.0, 0.0], [1e30, 0.0, 0.0], [0.0, 1e30, 0.0]);
        assert!(decimate(m, 100).triangles() <= 100);
    }
}
