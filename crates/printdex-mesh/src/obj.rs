//! Wavefront OBJ. Only geometry is read: `v` positions and `f` faces
//! (polygons are fan-triangulated). Materials, normals and texture
//! coordinates are ignored, and no `.mtl` or other file is ever opened.

use std::io::BufRead;

use crate::{Limits, Mesh, MeshError, Result, malformed};

pub(crate) fn read<R: BufRead>(mut r: R, limits: Limits) -> Result<Mesh> {
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut mesh = Mesh::default();
    let mut face: Vec<usize> = Vec::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        // OBJ is ASCII; tolerate stray non-UTF-8 bytes in comments.
        let text = String::from_utf8_lossy(&line);
        let mut tokens = text.split_ascii_whitespace();
        match tokens.next() {
            Some("v") => {
                let mut v = [0f32; 3];
                for c in &mut v {
                    *c = tokens
                        .next()
                        .and_then(|t| t.parse().ok())
                        .ok_or_else(|| malformed("bad vertex in OBJ"))?;
                }
                if verts.len() as u64 >= limits.max_triangles.saturating_mul(3) {
                    return Err(MeshError::TooLarge {
                        triangles: limits.max_triangles + 1,
                    });
                }
                verts.push(v);
            }
            Some("f") => {
                face.clear();
                for t in tokens {
                    // "7", "7/1", "7//3", "7/1/3"; negative is relative.
                    let raw: i64 = t
                        .split('/')
                        .next()
                        .and_then(|s| s.parse().ok())
                        .ok_or_else(|| malformed("bad face in OBJ"))?;
                    let idx = if raw > 0 {
                        usize::try_from(raw - 1).ok()
                    } else if raw < 0 {
                        verts.len().checked_sub(raw.unsigned_abs() as usize)
                    } else {
                        None
                    };
                    match idx {
                        Some(i) if i < verts.len() => face.push(i),
                        _ => return Err(malformed("face index out of range in OBJ")),
                    }
                }
                for k in 1..face.len().saturating_sub(1) {
                    if mesh.triangles() as u64 >= limits.max_triangles {
                        return Err(MeshError::TooLarge {
                            triangles: mesh.triangles() as u64 + 1,
                        });
                    }
                    mesh.push_triangle(verts[face[0]], verts[face[k]], verts[face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok(mesh)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load(s: &[u8]) -> Result<Mesh> {
        read(s, Limits::default())
    }

    #[test]
    fn reads_triangles() {
        let m = load(b"# cube corner\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n").unwrap();
        assert_eq!(m.triangles(), 1);
        assert_eq!(m.positions[1], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn triangulates_quads() {
        let m = load(b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1/1/1 2/2/1 3//1 4\n").unwrap();
        assert_eq!(m.triangles(), 2);
    }

    #[test]
    fn negative_indices_are_relative() {
        let m = load(b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf -3 -2 -1\n").unwrap();
        assert_eq!(m.triangles(), 1);
        assert_eq!(m.positions[0], [0.0, 0.0, 0.0]);
    }

    #[test]
    fn rejects_out_of_range_index() {
        assert!(load(b"v 0 0 0\nv 1 0 0\nf 1 2 3\n").is_err());
        assert!(load(b"v 0 0 0\nf 0 1 1\n").is_err());
        assert!(load(b"v 0 0 0\nf -4 1 1\n").is_err());
    }

    #[test]
    fn obj_never_panics() {
        let mut rng = fastrand::Rng::with_seed(11);
        let good = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 1\nf 1 2 3 4\nf -1 -2 -3\n".to_vec();
        for i in 0..1000 {
            let mut b = good.clone();
            if i % 2 == 0 {
                b.truncate(rng.usize(..=b.len()));
            } else {
                for _ in 0..6 {
                    let k = rng.usize(..b.len());
                    b[k] = rng.u8(..);
                }
            }
            let _ = load(&b);
        }
    }
}
