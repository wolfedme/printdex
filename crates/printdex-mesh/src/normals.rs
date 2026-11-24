//! Smooth corner normals for shading.
//!
//! Mesh files rarely carry usable normals (STL's are often zero or wrong),
//! so they are rebuilt here: identical corners are merged, and each
//! corner gets the area-weighted average of the faces around it that meet
//! its own face at less than [`CREASE_DEGREES`]. Curved surfaces then
//! shade smoothly while planar faces and hard edges stay crisp.

use crate::Mesh;

/// Faces meeting at a sharper angle than this keep a hard edge.
pub const CREASE_DEGREES: f32 = 35.0;

/// A unit normal packed as signed 8-bit components (x, y, z, 0), the
/// layout of a GPU `snorm8x4` vertex attribute.
pub type PackedNormal = [i8; 4];

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Packs a vector (any length) as a unit normal; zero stays zero.
pub fn pack(n: [f32; 3]) -> PackedNormal {
    let len = dot(n, n).sqrt();
    if len == 0.0 || !len.is_finite() {
        return [0; 4];
    }
    // Rounds by truncating a positive value (no libm call). `v / len` can
    // exceed 1 when `dot(n, n)` is subnormal and rounds down, so clamp
    // before narrowing: a tiny vector saturates rather than flipping sign.
    let c = |v: f32| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "float-to-int `as` saturates (NaN is 0); the result is clamped next"
        )]
        let r = (v / len * 127.0 + 127.5) as i32;
        i8::try_from(r.saturating_sub(127).clamp(-127, 127)).unwrap_or(0)
    };
    [c(n[0]), c(n[1]), c(n[2]), 0]
}

/// Unpacks a normal (not renormalised).
pub fn unpack(n: PackedNormal) -> [f32; 3] {
    [n[0], n[1], n[2]].map(|v| f32::from(v) / 127.0)
}

/// Vertex ids for every corner: identical positions share an id. Returns
/// the ids and the number of distinct positions.
///
/// An open-addressing table of ids, probed linearly; several times faster
/// than a general hash map for this one job. Needs fewer than `u32::MAX`
/// positions.
#[expect(
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    reason = "slot indices are the top bits of a hash (h >> (64 - bits)) or masked with \
              len - 1, below the power-of-two table length, so they fit usize and index in bounds"
)]
fn weld(positions: &[[f32; 3]]) -> (Vec<u32>, usize) {
    // A multiply-xorshift mix; tables take the high bits.
    let hash = |k: [u32; 3]| {
        let mut h = (u64::from(k[0]) | (u64::from(k[1]) << 32)).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        h ^= u64::from(k[2]).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
        h ^= h >> 29;
        h.wrapping_mul(0xd6e8_feb8_6659_fd93)
    };
    // Keys live in the table, so a probe touches one cache line. Triangle
    // soups repeat each position about six times.
    let mut slots: Vec<([u32; 3], u32)> =
        vec![([0; 3], 0); (positions.len() / 3).max(16).next_power_of_two()];
    // Most repeats of a position come within a few hundred corners: a
    // small direct-mapped cache in front of the table answers those
    // without touching the big table.
    const RECENT_BITS: u32 = 12;
    let mut recent: Vec<([u32; 3], u32)> = vec![([0; 3], 0); 1 << RECENT_BITS];
    let mut count = 0u32;
    let mut ids = Vec::with_capacity(positions.len());
    for p in positions {
        let k = [p[0].to_bits(), p[1].to_bits(), p[2].to_bits()];
        let h = hash(k);
        let r = (h >> (64 - RECENT_BITS)) as usize;
        if recent[r].1 != 0 && recent[r].0 == k {
            ids.push(recent[r].1 - 1);
            continue;
        }
        if count as usize * 2 >= slots.len() {
            let bigger = vec![([0; 3], 0); slots.len() * 2];
            let old = std::mem::replace(&mut slots, bigger);
            let (mask, shift) = (slots.len() - 1, 64 - slots.len().trailing_zeros());
            for (k, id) in old.into_iter().filter(|s| s.1 != 0) {
                let mut i = (hash(k) >> shift) as usize;
                while slots[i].1 != 0 {
                    i = (i + 1) & mask;
                }
                slots[i] = (k, id);
            }
        }
        let (mask, shift) = (slots.len() - 1, 64 - slots.len().trailing_zeros());
        let mut i = (h >> shift) as usize;
        let id = loop {
            let (have, id) = slots[i];
            if id == 0 {
                count += 1;
                slots[i] = (k, count);
                break count;
            }
            if have == k {
                break id;
            }
            i = (i + 1) & mask;
        };
        recent[r] = (k, id);
        ids.push(id - 1);
    }
    (ids, count as usize)
}

/// One smooth normal per corner of `mesh` (same order as
/// `mesh.positions`), in the winding of the corner's own face.
/// Degenerate faces, and the positions of a trailing partial triangle,
/// get a zero normal.
pub fn corner_normals(mesh: &Mesh) -> Vec<PackedNormal> {
    // Vertex and corner ids are `u32`: too many corners draw flat.
    if u32::try_from(mesh.positions.len()).is_err() {
        return vec![[0; 4]; mesh.positions.len()];
    }
    let tris = mesh.positions.as_chunks::<3>().0;
    // Only whole triangles: a partial one has no face to look up.
    let (ids, verts) = weld(tris.as_flattened());

    // Unit face normals and areas (twice the area; only ratios matter).
    let faces: Vec<([f32; 3], f32)> = tris
        .iter()
        .map(|t| {
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let len = dot(n, n).sqrt();
            if len > 0.0 && len.is_finite() {
                (n.map(|v| v / len), len)
            } else {
                ([0.0; 3], 0.0)
            }
        })
        .collect();

    // The corners at each vertex, as one flat list with offsets: vertex
    // `v`'s corners are `around[start[v]..start[v + 1]]`. (Every id is
    // below `verts`, so the lookups below always succeed.)
    let mut start = vec![0u32; verts + 1];
    for &v in &ids {
        if let Some(n) = start.get_mut(v as usize + 1) {
            *n += 1;
        }
    }
    let mut total = 0;
    for s in &mut start {
        total += *s;
        *s = total;
    }
    let mut fill = start.clone();
    let mut around = vec![0u32; ids.len()];
    for (corner, &v) in (0u32..).zip(&ids) {
        if let Some(slot) = fill.get_mut(v as usize) {
            if let Some(a) = around.get_mut(*slot as usize) {
                *a = corner;
            }
            *slot += 1;
        }
    }
    drop(fill);
    drop(ids);

    // Per vertex: each corner sums the faces there that meet its own face
    // at less than the crease angle.
    let crease = CREASE_DEGREES.to_radians().cos();
    let mut out = vec![[0i8; 4]; mesh.positions.len()];
    let mut local: Vec<([f32; 3], f32)> = Vec::new();
    for span in start.windows(2) {
        let &[from, to] = span else {
            continue;
        };
        let corners = around.get(from as usize..to as usize).unwrap_or_default();
        local.clear();
        local.extend(corners.iter().map(|&c| {
            // A missing face counts as degenerate.
            faces
                .get(c as usize / 3)
                .copied()
                .unwrap_or(([0.0; 3], 0.0))
        }));
        for (&c, &(own, area)) in corners.iter().zip(&local) {
            if area == 0.0 {
                continue;
            }
            let mut sum = [0.0f32; 3];
            for &(n, a) in &local {
                if dot(n, own) >= crease {
                    for (s, v) in sum.iter_mut().zip(n) {
                        *s += v * a;
                    }
                }
            }
            if let Some(o) = out.get_mut(c as usize) {
                *o = pack(sum);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(m: &mut Mesh, a: [f32; 3], b: [f32; 3], c: [f32; 3], d: [f32; 3]) {
        m.positions.extend([a, b, c, a, c, d]);
    }

    #[test]
    fn planar_faces_keep_their_normal() {
        // Two faces of a box meeting at 90°: a hard edge.
        let mut m = Mesh::default();
        quad(
            &mut m,
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        );
        quad(
            &mut m,
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
        );
        let n = corner_normals(&m);
        for c in &n[..6] {
            assert_eq!(*c, [0, 0, 127, 0]);
        }
        for c in &n[6..] {
            assert_eq!(*c, [0, 127, 0, 0]);
        }
    }

    #[test]
    fn shallow_angles_are_smoothed() {
        // Two faces meeting at 20°: the shared corners get the average.
        let t = 20.0f32.to_radians();
        let mut m = Mesh::default();
        m.positions
            .extend([[-1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
        m.positions
            .extend([[0.0, 0.0, 0.0], [t.cos(), 0.0, t.sin()], [0.0, 1.0, 0.0]]);
        let n = corner_normals(&m);
        // Corner 1 is shared: tilted halfway, about 10°.
        let shared = unpack(n[1]);
        let angle = shared[2].acos().to_degrees();
        assert!((angle - 10.0).abs() < 2.0, "{angle}");
        // Corner 0 is only on the flat face.
        assert_eq!(n[0], [0, 0, 127, 0]);
    }

    #[test]
    fn degenerate_faces_get_zero() {
        let m = Mesh {
            positions: vec![[0.0; 3], [0.0; 3], [1.0, 0.0, 0.0]],
        };
        assert_eq!(corner_normals(&m), vec![[0; 4]; 3]);
    }

    #[test]
    fn tiny_vectors_keep_their_sign() {
        // dot(n, n) is subnormal and rounds down, so v / len exceeds 1.
        assert_eq!(pack([4e-23, 0.0, 0.0]), [127, 0, 0, 0]);
        assert_eq!(pack([-4e-23, 0.0, 0.0]), [-127, 0, 0, 0]);
        let [x, y, z, _] = pack([0.0, 4e-23, -4e-23]);
        assert!(x == 0 && y > 0 && z < 0, "{x} {y} {z}");
    }

    #[test]
    fn trailing_positions_of_a_partial_triangle_get_zero() {
        // A mesh built by hand can end with a partial triangle.
        let mut m = Mesh {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        };
        m.positions.extend([[5.0, 5.0, 5.0], [0.0, 0.0, 0.0]]);
        let n = corner_normals(&m);
        assert_eq!(n.len(), 5);
        assert_eq!(&n[..3], &[[0, 0, 127, 0]; 3]);
        assert_eq!(&n[3..], &[[0; 4]; 2]);
        for short in 1..3 {
            let m = Mesh {
                positions: vec![[1.0; 3]; short],
            };
            assert_eq!(corner_normals(&m), vec![[0; 4]; short]);
        }
        assert!(corner_normals(&Mesh::default()).is_empty());
    }
}
