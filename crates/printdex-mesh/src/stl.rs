//! STL, binary and ASCII. Stored normals are ignored; renderers compute
//! their own from the winding.

use std::io::{BufRead, Read};

use crate::{Limits, Mesh, MeshError, Result, malformed};

const HEADER: u64 = 80;
/// Bytes per binary triangle record.
const RECORD: usize = 50;
/// Records decoded per read call.
const CHUNK_RECORDS: usize = 4096;

/// Reads an STL of `size` bytes from `r`.
///
/// Binary when the size matches the triangle count in the header (even if
/// the header starts with "solid", which many exporters write), ASCII when
/// the file starts with "solid", otherwise binary with trailing bytes
/// ignored, or an error when the file is shorter than its count says.
pub(crate) fn read<R: BufRead>(mut r: R, size: u64, limits: Limits) -> Result<Mesh> {
    let mut head = [0u8; 84];
    let got = read_up_to(&mut r, &mut head)?;
    // `read_up_to` never reports more than it was given room for.
    let start = head.get(..got).unwrap_or_default();
    let starts_solid = start
        .iter()
        .skip_while(|b| b.is_ascii_whitespace())
        .take(5)
        .eq(b"solid".iter());
    if got == 84 {
        let count = u64::from(u32::from_le_bytes([head[80], head[81], head[82], head[83]]));
        let expected = HEADER + 4 + count * RECORD as u64;
        if expected == size || (!starts_solid && expected < size) {
            return read_binary(r, count, limits);
        }
        if !starts_solid {
            return Err(malformed(format!(
                "binary STL declares {count} triangles but the file holds {size} bytes"
            )));
        }
    } else if !starts_solid {
        return Err(malformed("file too short for a binary STL"));
    }
    read_ascii(start.chain(r), limits)
}

/// Fills `buf` as far as the reader allows; returns the bytes read.
fn read_up_to<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while let Some(rest) = buf.get_mut(n..).filter(|rest| !rest.is_empty()) {
        match r.read(rest) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    // A reader may claim more than it was given room for.
    Ok(n.min(buf.len()))
}

fn read_binary<R: Read>(mut r: R, count: u64, limits: Limits) -> Result<Mesh> {
    if count > limits.max_triangles {
        return Err(MeshError::TooLarge { triangles: count });
    }
    let count = usize::try_from(count).map_err(|_| MeshError::TooLarge { triangles: count })?;
    let mut mesh = Mesh {
        positions: Vec::with_capacity(count * 3),
    };
    let mut buf = vec![0u8; CHUNK_RECORDS * RECORD];
    let mut left = count;
    while left > 0 {
        let n = left.min(CHUNK_RECORDS);
        #[expect(
            clippy::indexing_slicing,
            reason = "n <= CHUNK_RECORDS and buf holds CHUNK_RECORDS records"
        )]
        let bytes = &mut buf[..n * RECORD];
        r.read_exact(bytes).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                malformed("binary STL ends early")
            } else {
                e.into()
            }
        })?;
        for rec in bytes.as_chunks::<RECORD>().0 {
            // 12 bytes normal, 3 × 12 bytes vertices, 2 bytes attribute.
            let [_, _, _, ax, ay, az, bx, by, bz, cx, cy, cz] = *rec.as_chunks::<4>().0 else {
                continue;
            };
            let f = f32::from_le_bytes;
            mesh.push_triangle(
                [f(ax), f(ay), f(az)],
                [f(bx), f(by), f(bz)],
                [f(cx), f(cy), f(cz)],
            );
        }
        left -= n;
    }
    Ok(mesh)
}

fn read_ascii<R: BufRead>(mut r: R, limits: Limits) -> Result<Mesh> {
    let mut mesh = Mesh::default();
    let mut verts: Vec<[f32; 3]> = Vec::with_capacity(3);
    let mut line = Vec::new();
    loop {
        line.clear();
        if r.read_until(b'\n', &mut line)? == 0 {
            break;
        }
        let text = std::str::from_utf8(&line).map_err(|_| malformed("ASCII STL is not text"))?;
        let mut tokens = text.split_ascii_whitespace();
        match tokens.next() {
            Some("vertex") => {
                let mut v = [0f32; 3];
                for c in &mut v {
                    *c = tokens
                        .next()
                        .and_then(|t| t.parse().ok())
                        .ok_or_else(|| malformed("bad vertex in ASCII STL"))?;
                }
                verts.push(v);
                if let [a, b, c] = *verts.as_slice() {
                    if mesh.triangles() as u64 >= limits.max_triangles {
                        return Err(MeshError::TooLarge {
                            triangles: mesh.triangles() as u64 + 1,
                        });
                    }
                    mesh.push_triangle(a, b, c);
                    verts.clear();
                }
            }
            Some("endloop") => verts.clear(),
            _ => {}
        }
    }
    Ok(mesh)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn binary(header: &[u8], tris: &[[[f32; 3]; 3]]) -> Vec<u8> {
        let mut out = vec![0u8; 80];
        out[..header.len()].copy_from_slice(header);
        out.extend((tris.len() as u32).to_le_bytes());
        for t in tris {
            out.extend([0u8; 12]);
            for v in t {
                for c in v {
                    out.extend(c.to_le_bytes());
                }
            }
            out.extend([0u8; 2]);
        }
        out
    }

    fn load(bytes: &[u8]) -> Result<Mesh> {
        read(bytes, bytes.len() as u64, Limits::default())
    }

    const TRI: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 2.0]];

    #[test]
    fn binary_stl_reads_triangles() {
        let m = load(&binary(b"part", &[TRI, TRI])).unwrap();
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.positions[2], [0.0, 1.0, 2.0]);
    }

    #[test]
    fn ascii_stl_reads_triangles() {
        let s = "solid x\n facet normal 0 0 1\n  outer loop\n   vertex 0 0 0\n   vertex 1 0 0\n   vertex 0 1 2.5e0\n  endloop\n endfacet\nendsolid x\n";
        let m = load(s.as_bytes()).unwrap();
        assert_eq!(m.triangles(), 1);
        assert_eq!(m.positions[2], [0.0, 1.0, 2.5]);
    }

    #[test]
    fn binary_stl_starting_with_solid_is_binary() {
        let m = load(&binary(b"solid exported by CAD", &[TRI])).unwrap();
        assert_eq!(m.triangles(), 1);
    }

    #[test]
    fn rejects_truncated_binary() {
        let mut b = binary(b"", &[TRI, TRI]);
        b.truncate(b.len() - 10);
        assert!(matches!(load(&b), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn rejects_count_larger_than_file() {
        let mut b = binary(b"", &[TRI]);
        b[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(load(&b).is_err());
    }

    #[test]
    fn refuses_more_triangles_than_the_limit() {
        let b = binary(b"", &[TRI, TRI, TRI]);
        let limits = Limits {
            max_triangles: 2,
            ..Limits::default()
        };
        assert!(matches!(
            read(b.as_slice(), b.len() as u64, limits),
            Err(MeshError::TooLarge { triangles: 3 })
        ));
    }

    #[test]
    fn trailing_bytes_after_binary_are_ignored() {
        let mut b = binary(b"", &[TRI]);
        b.extend([7u8; 30]);
        assert_eq!(load(&b).unwrap().triangles(), 1);
    }

    #[test]
    fn drops_non_finite_triangles() {
        let bad = [[f32::NAN, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        assert_eq!(load(&binary(b"", &[bad, TRI])).unwrap().triangles(), 1);
    }

    /// A reader that claims to have read more than it was given room for.
    struct Liar;

    impl Read for Liar {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            Ok(buf.len() + 10)
        }
    }

    impl BufRead for Liar {
        fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
            Ok(&[])
        }

        fn consume(&mut self, _: usize) {}
    }

    #[test]
    fn a_reader_reporting_too_much_is_not_trusted() {
        let mut buf = [0u8; 4];
        assert_eq!(read_up_to(&mut Liar, &mut buf).unwrap(), 4);
        // The header read claims 94 of 84 bytes: taken as 84 (all zero,
        // so a binary STL of no triangles).
        assert_eq!(read(Liar, 84, Limits::default()).unwrap().triangles(), 0);
    }

    #[test]
    fn stl_never_panics() {
        let mut rng = fastrand::Rng::with_seed(7);
        let good = binary(b"", &[TRI; 4]);
        let ascii = b"solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid\n";
        for i in 0..1000 {
            let mut b = if i % 2 == 0 {
                good.clone()
            } else {
                ascii.to_vec()
            };
            match i % 3 {
                0 => b.truncate(rng.usize(..=b.len())),
                1 => {
                    for _ in 0..8 {
                        let k = rng.usize(..b.len());
                        b[k] = rng.u8(..);
                    }
                }
                _ => b = (0..rng.usize(..300)).map(|_| rng.u8(..)).collect(),
            }
            let _ = load(&b);
        }
    }
}
