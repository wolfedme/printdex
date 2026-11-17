//! 3MF: a zip package with XML model parts.
//!
//! Supports mesh objects, components (including the production extension's
//! components in other model files, which Bambu Studio and OrcaSlicer
//! write), build item transforms, model units, and the package thumbnail.

use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use quick_xml::events::{BytesStart, Event};

use crate::{Limits, Mesh, MeshError, READ_BUFFER, Result, malformed};

/// Component nesting deeper than this is treated as a cycle.
const MAX_DEPTH: u32 = 16;
/// Most model files one package may reference.
const MAX_MODEL_FILES: usize = 4096;
/// Largest embedded thumbnail read, in bytes.
const MAX_THUMBNAIL: u64 = 16 << 20;
const MODEL_REL: &str = "/3dmodel";
const THUMB_REL: &str = "/thumbnail";

/// Affine transform in 3MF's row-vector layout: `m00 m01 m02 m10 … m32`.
type Mat = [f32; 12];
const IDENTITY: Mat = [1., 0., 0., 0., 1., 0., 0., 0., 1., 0., 0., 0.];

fn apply(m: &Mat, p: [f32; 3]) -> [f32; 3] {
    [
        p[0] * m[0] + p[1] * m[3] + p[2] * m[6] + m[9],
        p[0] * m[1] + p[1] * m[4] + p[2] * m[7] + m[10],
        p[0] * m[2] + p[1] * m[5] + p[2] * m[8] + m[11],
    ]
}

/// `a` then `b`.
fn then(a: &Mat, b: &Mat) -> Mat {
    let mut out = [0f32; 12];
    for row in 0..4 {
        for col in 0..3 {
            let mut v = if row == 3 { b[9 + col] } else { 0.0 };
            for k in 0..3 {
                v += a[row * 3 + k] * b[k * 3 + col];
            }
            out[row * 3 + col] = v;
        }
    }
    out
}

enum Object {
    Mesh {
        verts: Vec<[f32; 3]>,
        tris: Vec<[u32; 3]>,
    },
    Components(Vec<Component>),
}

struct Component {
    /// Model file holding the object; `None` is the referencing file.
    path: Option<String>,
    id: u32,
    transform: Mat,
}

#[derive(Default)]
struct ModelFile {
    objects: HashMap<u32, Object>,
    build: Vec<(u32, Mat)>,
    /// Millimetres per model unit.
    scale: f32,
}

/// Reads an entry with a size cap, so a zip bomb fails instead of filling
/// memory.
struct Capped<R> {
    inner: R,
    left: u64,
}

impl<R: Read> Read for Capped<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "archive entry larger than the limit",
            ));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

fn open(path: &Path) -> Result<zip::ZipArchive<BufReader<File>>> {
    let file = BufReader::with_capacity(READ_BUFFER, File::open(path)?);
    zip::ZipArchive::new(file).map_err(zip_err)
}

fn zip_err(e: zip::result::ZipError) -> MeshError {
    match e {
        zip::result::ZipError::Io(e) => MeshError::Io(e),
        other => malformed(format!("bad 3MF package: {other}")),
    }
}

/// Entry name for a package path such as `/3D/3dmodel.model`.
fn entry_name<R: Read + Seek>(zip: &zip::ZipArchive<R>, target: &str) -> Option<String> {
    let want = target.trim_start_matches('/');
    if zip.index_for_name(want).is_some() {
        return Some(want.to_owned());
    }
    zip.file_names()
        .flatten()
        .find(|n| n.eq_ignore_ascii_case(want))
        .map(|n| n.into_owned())
}

fn read_entry<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    cap: u64,
) -> Result<Vec<u8>> {
    let entry = zip.by_name(name).map_err(zip_err)?;
    let mut out = Vec::new();
    Capped {
        inner: entry,
        left: cap,
    }
    .read_to_end(&mut out)
    .map_err(|e| malformed(e.to_string()))?;
    Ok(out)
}

fn attr(e: &BytesStart<'_>, name: &[u8]) -> Option<String> {
    e.attributes()
        .flatten()
        .find(|a| a.key.local_name().as_ref() == name)
        .and_then(|a| String::from_utf8(a.value.into_owned()).ok())
}

fn num<T: std::str::FromStr>(e: &BytesStart<'_>, name: &[u8]) -> Option<T> {
    attr(e, name).and_then(|v| v.trim().parse().ok())
}

fn transform(e: &BytesStart<'_>) -> Result<Mat> {
    let Some(s) = attr(e, b"transform") else {
        return Ok(IDENTITY);
    };
    let vals: Vec<f32> = s
        .split_ascii_whitespace()
        .map(|t| t.parse::<f32>())
        .collect::<std::result::Result<_, _>>()
        .map_err(|_| malformed("bad transform in 3MF"))?;
    let m: Mat = vals
        .try_into()
        .map_err(|_| malformed("transform needs 12 values"))?;
    if m.iter().all(|v| v.is_finite()) {
        Ok(m)
    } else {
        Err(malformed("non-finite transform in 3MF"))
    }
}

/// Relationship targets of the package: (model part, thumbnail).
fn package_rels<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    limits: Limits,
) -> Result<(Option<String>, Option<String>)> {
    let Some(name) = entry_name(zip, "_rels/.rels") else {
        return Ok((None, None));
    };
    let bytes = read_entry(zip, &name, limits.max_entry_bytes.min(1 << 20))?;
    let mut reader = quick_xml::Reader::from_reader(bytes.as_slice());
    let mut buf = Vec::new();
    let (mut model, mut thumb) = (None, None);
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e) | Event::Empty(e)) if e.local_name().as_ref() == b"Relationship" => {
                let kind = attr(&e, b"Type").unwrap_or_default();
                let target = attr(&e, b"Target");
                if kind.ends_with(MODEL_REL) && model.is_none() {
                    model = target;
                } else if kind.ends_with(THUMB_REL) && thumb.is_none() {
                    thumb = target;
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(malformed(format!("bad 3MF relationships: {e}"))),
            _ => {}
        }
        buf.clear();
    }
    Ok((model, thumb))
}

fn unit_scale(unit: Option<String>) -> f32 {
    match unit.as_deref() {
        Some("micron") => 0.001,
        Some("centimeter") => 10.0,
        Some("inch") => 25.4,
        Some("foot") => 304.8,
        Some("meter") => 1000.0,
        _ => 1.0,
    }
}

fn parse_model<R: Read + Seek>(
    zip: &mut zip::ZipArchive<R>,
    name: &str,
    limits: Limits,
    triangles: &mut u64,
) -> Result<ModelFile> {
    let entry = zip.by_name(name).map_err(zip_err)?;
    let capped = Capped {
        inner: entry,
        left: limits.max_entry_bytes,
    };
    let mut reader = quick_xml::Reader::from_reader(BufReader::with_capacity(1 << 16, capped));
    let mut buf = Vec::new();
    let mut file = ModelFile {
        scale: 1.0,
        ..ModelFile::default()
    };
    // Object being read: id, vertices, triangles, components.
    let mut current: Option<u32> = None;
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut tris: Vec<[u32; 3]> = Vec::new();
    let mut comps: Vec<Component> = Vec::new();
    let mut in_build = false;
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .map_err(|e| malformed(format!("bad 3MF model: {e}")))?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => match e.local_name().as_ref() {
                b"model" => file.scale = unit_scale(attr(e, b"unit")),
                b"object" => {
                    current = num(e, b"id");
                    verts.clear();
                    tris.clear();
                    comps.clear();
                }
                b"vertex" if current.is_some() => {
                    let v = [num(e, b"x"), num(e, b"y"), num(e, b"z")];
                    match v {
                        [Some(x), Some(y), Some(z)] => verts.push([x, y, z]),
                        _ => return Err(malformed("bad vertex in 3MF")),
                    }
                }
                b"triangle" if current.is_some() => {
                    let t = [num(e, b"v1"), num(e, b"v2"), num(e, b"v3")];
                    let [Some(a), Some(b), Some(c)] = t else {
                        return Err(malformed("bad triangle in 3MF"));
                    };
                    *triangles += 1;
                    if *triangles > limits.max_triangles {
                        return Err(MeshError::TooLarge {
                            triangles: *triangles,
                        });
                    }
                    tris.push([a, b, c]);
                }
                b"component" if current.is_some() => {
                    let Some(id) = num(e, b"objectid") else {
                        return Err(malformed("component without object id"));
                    };
                    comps.push(Component {
                        path: attr(e, b"path"),
                        id,
                        transform: transform(e)?,
                    });
                }
                b"build" => in_build = true,
                b"item" if in_build => {
                    if let Some(id) = num(e, b"objectid") {
                        file.build.push((id, transform(e)?));
                    }
                }
                _ => {}
            },
            Event::End(ref e) => match e.local_name().as_ref() {
                b"object" => {
                    if let Some(id) = current.take() {
                        let n = verts.len();
                        if tris.iter().flatten().any(|&i| i as usize >= n) {
                            return Err(malformed("triangle index out of range in 3MF"));
                        }
                        let obj = if comps.is_empty() {
                            Object::Mesh {
                                verts: std::mem::take(&mut verts),
                                tris: std::mem::take(&mut tris),
                            }
                        } else {
                            Object::Components(std::mem::take(&mut comps))
                        };
                        file.objects.insert(id, obj);
                    }
                }
                b"build" => in_build = false,
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(file)
}

fn flatten(
    files: &HashMap<String, ModelFile>,
    file: &str,
    id: u32,
    mat: &Mat,
    depth: u32,
    out: &mut Mesh,
) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(malformed("components nested too deep in 3MF"));
    }
    let Some(model) = files.get(file) else {
        return Err(malformed(format!("missing model file {file}")));
    };
    let Some(obj) = model.objects.get(&id) else {
        return Err(malformed(format!("missing object {id} in 3MF")));
    };
    match obj {
        Object::Mesh { verts, tris } => {
            let s = model.scale;
            let p = |i: u32| {
                let v = verts[i as usize];
                apply(mat, [v[0] * s, v[1] * s, v[2] * s])
            };
            out.positions.reserve(tris.len() * 3);
            for t in tris {
                out.push_triangle(p(t[0]), p(t[1]), p(t[2]));
            }
        }
        Object::Components(comps) => {
            for c in comps {
                let target = c.path.as_deref().map_or(file.to_owned(), normalize);
                // A component's transform is in its parent's units.
                let mut local = c.transform;
                let child_scale = files.get(&target).map_or(1.0, |f| f.scale);
                if child_scale != 0.0 && model.scale != child_scale {
                    for v in &mut local[9..] {
                        *v *= model.scale / child_scale;
                    }
                }
                flatten(files, &target, c.id, &then(&local, mat), depth + 1, out)?;
            }
        }
    }
    Ok(())
}

fn normalize(p: &str) -> String {
    p.trim_start_matches('/').to_owned()
}

/// Reads all build items of the 3MF at `path` into one mesh.
pub(crate) fn read(path: &Path, limits: Limits) -> Result<Mesh> {
    read_from(open(path)?, limits)
}

fn read_from<R: Read + Seek>(mut zip: zip::ZipArchive<R>, limits: Limits) -> Result<Mesh> {
    let (model_rel, _) = package_rels(&mut zip, limits)?;
    let root = model_rel
        .as_deref()
        .and_then(|t| entry_name(&zip, t))
        .or_else(|| entry_name(&zip, "3D/3dmodel.model"))
        .ok_or_else(|| malformed("3MF has no model part"))?;

    let mut triangles = 0u64;
    let mut files: HashMap<String, ModelFile> = HashMap::new();
    let mut queue = VecDeque::from([root.clone()]);
    while let Some(name) = queue.pop_front() {
        if files.contains_key(&name) {
            continue;
        }
        if files.len() >= MAX_MODEL_FILES {
            return Err(malformed("too many model files in 3MF"));
        }
        let Some(entry) = entry_name(&zip, &name) else {
            return Err(malformed(format!("missing model file {name}")));
        };
        let file = parse_model(&mut zip, &entry, limits, &mut triangles)?;
        for obj in file.objects.values() {
            if let Object::Components(comps) = obj {
                for c in comps {
                    if let Some(p) = &c.path {
                        queue.push_back(normalize(p));
                    }
                }
            }
        }
        files.insert(name, file);
    }

    let mut mesh = Mesh::default();
    let build = files
        .get(&root)
        .map(|f| f.build.clone())
        .unwrap_or_default();
    for (id, mat) in build {
        flatten(&files, &root, id, &mat, 0, &mut mesh)?;
    }
    Ok(mesh)
}

/// The package thumbnail of the 3MF at `path` (PNG or JPEG bytes): the
/// one its relationships name, else a slicer plate image. `None` when the
/// package has none.
pub fn thumbnail(path: &Path, limits: Limits) -> Result<Option<Vec<u8>>> {
    thumbnail_from(open(path)?, limits)
}

fn thumbnail_from<R: Read + Seek>(
    mut zip: zip::ZipArchive<R>,
    limits: Limits,
) -> Result<Option<Vec<u8>>> {
    let (_, rel) = package_rels(&mut zip, limits)?;
    let candidates = rel.into_iter().chain([
        "Metadata/plate_1.png".into(),
        "Metadata/thumbnail.png".into(),
    ]);
    for c in candidates {
        if let Some(name) = entry_name(&zip, &c) {
            let cap = limits.max_entry_bytes.min(MAX_THUMBNAIL);
            return read_entry(&mut zip, &name, cap).map(Some);
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel0" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
 <Relationship Target="/Metadata/thumbnail.png" Id="rel1" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
</Relationships>"#;

    fn tetra(id: u32) -> String {
        format!(
            r#"<object id="{id}" type="model"><mesh><vertices>
<vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/><vertex x="0" y="0" z="10"/>
</vertices><triangles>
<triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="1" v3="3"/><triangle v1="1" v2="2" v3="3"/><triangle v1="0" v2="3" v3="2"/>
</triangles></mesh></object>"#
        )
    }

    fn model(resources: &str, build: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06">
<resources>{resources}</resources><build>{build}</build></model>"#
        )
    }

    fn package(entries: &[(&str, &[u8])]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(bytes).unwrap();
        }
        let cursor = w.finish().unwrap();
        zip::ZipArchive::new(Cursor::new(cursor.into_inner())).unwrap()
    }

    fn simple(m: &str) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        package(&[
            ("_rels/.rels", RELS.as_bytes()),
            ("3D/3dmodel.model", m.as_bytes()),
        ])
    }

    #[test]
    fn reads_mesh_object() {
        let m = read_from(
            simple(&model(&tetra(1), r#"<item objectid="1"/>"#)),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(m.triangles(), 4);
        assert_eq!(m.bounds().unwrap().size(), [10.0, 10.0, 10.0]);
    }

    #[test]
    fn applies_build_item_transform() {
        let m = read_from(
            simple(&model(
                &tetra(1),
                r#"<item objectid="1" transform="2 0 0 0 1 0 0 0 1 100 0 5"/>"#,
            )),
            Limits::default(),
        )
        .unwrap();
        let b = m.bounds().unwrap();
        assert_eq!(b.min, [100.0, 0.0, 5.0]);
        assert_eq!(b.size(), [20.0, 10.0, 10.0]);
    }

    #[test]
    fn resolves_components() {
        let res = format!(
            r#"{}<object id="2"><components><component objectid="1"/><component objectid="1" transform="1 0 0 0 1 0 0 0 1 50 0 0"/></components></object>"#,
            tetra(1)
        );
        let m = read_from(
            simple(&model(&res, r#"<item objectid="2"/>"#)),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(m.triangles(), 8);
        assert_eq!(m.bounds().unwrap().size()[0], 60.0);
    }

    #[test]
    fn resolves_components_in_other_model_files() {
        let root = model(
            r#"<object id="5"><components><component p:path="/3D/Objects/object_1.model" objectid="1" transform="1 0 0 0 1 0 0 0 1 0 0 20"/></components></object>"#,
            r#"<item objectid="5"/>"#,
        );
        let child = model(&tetra(1), "");
        let m = read_from(
            package(&[
                ("_rels/.rels", RELS.as_bytes()),
                ("3D/3dmodel.model", root.as_bytes()),
                ("3D/Objects/object_1.model", child.as_bytes()),
            ]),
            Limits::default(),
        )
        .unwrap();
        assert_eq!(m.triangles(), 4);
        assert_eq!(m.bounds().unwrap().min[2], 20.0);
    }

    #[test]
    fn component_cycles_fail() {
        let res = r#"<object id="1"><components><component objectid="2"/></components></object><object id="2"><components><component objectid="1"/></components></object>"#;
        assert!(
            read_from(
                simple(&model(res, r#"<item objectid="1"/>"#)),
                Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn honours_model_units() {
        let m = model(&tetra(1), r#"<item objectid="1"/>"#).replace("millimeter", "centimeter");
        let mesh = read_from(simple(&m), Limits::default()).unwrap();
        assert_eq!(mesh.bounds().unwrap().size(), [100.0, 100.0, 100.0]);
    }

    #[test]
    fn finds_thumbnail_via_rels() {
        let zip = package(&[
            ("_rels/.rels", RELS.as_bytes()),
            ("Metadata/thumbnail.png", b"PNGDATA"),
            ("Metadata/plate_1.png", b"PLATE"),
        ]);
        assert_eq!(
            thumbnail_from(zip, Limits::default()).unwrap().as_deref(),
            Some(b"PNGDATA".as_slice())
        );
    }

    #[test]
    fn finds_bambu_plate_thumbnail() {
        let zip = package(&[("Metadata/plate_1.png", b"PLATE")]);
        assert_eq!(
            thumbnail_from(zip, Limits::default()).unwrap().as_deref(),
            Some(b"PLATE".as_slice())
        );
        let none = package(&[("3D/3dmodel.model", b"<model/>")]);
        assert_eq!(thumbnail_from(none, Limits::default()).unwrap(), None);
    }

    #[test]
    fn rejects_missing_model_part() {
        let zip = package(&[("_rels/.rels", RELS.as_bytes())]);
        assert!(matches!(
            read_from(zip, Limits::default()),
            Err(MeshError::Malformed(_))
        ));
    }

    #[test]
    fn caps_decompressed_size() {
        let big = model(&tetra(1), r#"<item objectid="1"/>"#) + &" ".repeat(100_000);
        let limits = Limits {
            max_entry_bytes: 10_000,
            ..Limits::default()
        };
        let err = read_from(simple(&big), limits).unwrap_err();
        assert!(err.to_string().contains("limit"), "{err}");
    }

    #[test]
    fn rejects_bad_indices() {
        let res = tetra(1).replace(
            r#"v3="3"/><triangle v1="0" v2="3""#,
            r#"v3="9"/><triangle v1="0" v2="3""#,
        );
        assert!(
            read_from(
                simple(&model(&res, r#"<item objectid="1"/>"#)),
                Limits::default()
            )
            .is_err()
        );
    }

    #[test]
    fn refuses_more_triangles_than_the_limit() {
        let limits = Limits {
            max_triangles: 3,
            ..Limits::default()
        };
        assert!(matches!(
            read_from(simple(&model(&tetra(1), r#"<item objectid="1"/>"#)), limits),
            Err(MeshError::TooLarge { .. })
        ));
    }

    #[test]
    fn threemf_never_panics() {
        let mut rng = fastrand::Rng::with_seed(3);
        let good = model(&tetra(1), r#"<item objectid="1"/>"#).into_bytes();
        for _ in 0..500 {
            let mut b = good.clone();
            for _ in 0..4 {
                let k = rng.usize(..b.len());
                b[k] = rng.u8(..);
            }
            b.truncate(rng.usize(b.len() / 2..=b.len()));
            let _ = read_from(
                package(&[("_rels/.rels", RELS.as_bytes()), ("3D/3dmodel.model", &b)]),
                Limits::default(),
            );
        }
        // Not a zip at all.
        let junk: Vec<u8> = (0..500).map(|_| rng.u8(..)).collect();
        assert!(zip::ZipArchive::new(Cursor::new(junk)).is_err());
    }
}
