//! CPU rasterizer for thumbnails: triangles with smooth normals into a
//! z-buffer at 2×2 supersampling, then shading in the material ramp with
//! a soft highlight, ambient occlusion from the depth buffer, an edge line
//! where the model meets the background or a farther surface, and
//! downsampling in linear light.
//!
//! No GPU is needed, so thumbnails render the same on every machine,
//! including CI runners and remote desktops without a capable GPU.

use glam::{Vec2, Vec3};
use printdex_mesh::Mesh;

use printdex_mesh::{PackedNormal, unpack};

use crate::camera::{Basis, Camera, Fit, Framing, Margins};
use crate::material::{AO_RADIUS, AO_SAMPLES, Lighting, Material, SrgbTable, ao_kernel, ao_turn};

/// Supersampling factor per axis.
const SS: u32 = 2;
/// Edge line width in supersampled pixels (one output pixel).
const EDGE_PX: i64 = 2;

/// An RGBA image, 8 bits per channel, straight (not premultiplied) alpha,
/// rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn transparent(width: u32, height: u32) -> Image {
        Image {
            width,
            height,
            rgba: vec![0; (width * height * 4) as usize],
        }
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

/// What to render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub material: Material,
    pub camera: Camera,
    /// Where the model sits in the image.
    pub framing: Framing,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            width: 384,
            height: 288,
            material: Material::Unknown,
            camera: Camera::default(),
            framing: Framing::OnBed(Margins::THUMBNAIL),
        }
    }
}

/// Supersampled depth and normal buffers. Normals are packed as signed
/// bytes (like the GPU path's normal target) and not yet unit length.
pub(crate) struct Target {
    pub w: u32,
    pub h: u32,
    pub depth: Vec<f32>,
    pub normal: Vec<[i8; 4]>,
}

/// Packs a normal of length up to 1 into a [`Target`] pixel.
pub(crate) fn pack_px(n: Vec3) -> [i8; 4] {
    // `as` saturates, so a slightly long vector stays in range.
    [
        (n.x * 127.0) as i8,
        (n.y * 127.0) as i8,
        (n.z * 127.0) as i8,
        0,
    ]
}

/// The unit normal of a [`Target`] pixel.
fn unpack_px(n: [i8; 4]) -> Vec3 {
    Vec3::new(f32::from(n[0]), f32::from(n[1]), f32::from(n[2])).normalize_or_zero()
}

impl Target {
    pub fn new(w: u32, h: u32) -> Target {
        let n = (w * h) as usize;
        Target {
            w,
            h,
            depth: vec![f32::NEG_INFINITY; n],
            normal: vec![[0; 4]; n],
        }
    }

    fn covered(&self, i: usize) -> bool {
        self.depth[i] > f32::NEG_INFINITY
    }

    /// Fills the triangle with pixel corners `p`, depths `d` (larger is
    /// nearer) and corner normals `n` where it is nearer than what is there.
    pub fn triangle(&mut self, p: [Vec2; 3], d: [f32; 3], n: [Vec3; 3]) {
        let edge = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
        let area = edge(p[0], p[1], p[2]);
        if area == 0.0 || !area.is_finite() {
            return;
        }
        let min = p[0].min(p[1]).min(p[2]);
        let max = p[0].max(p[1]).max(p[2]);
        let x0 = (min.x.floor().max(0.0)) as u32;
        let y0 = (min.y.floor().max(0.0)) as u32;
        let x1 = (max.x.ceil().min(self.w as f32)) as u32;
        let y1 = (max.y.ceil().min(self.h as f32)) as u32;
        let inv = 1.0 / area;
        // Flat triangles (all corners alike) pack their normal once.
        let flat = (n[0] == n[1] && n[1] == n[2]).then(|| pack_px(n[0]));
        // Each barycentric weight is a plane over the image: its value at
        // the first pixel centre of a row, plus a step per pixel.
        let plane = |a: Vec2, b: Vec2| {
            let (dx, dy) = ((b.x - a.x) * inv, (b.y - a.y) * inv);
            let at = move |x: f32, y: f32| dx * (y - a.y) - dy * (x - a.x);
            (at, -dy)
        };
        let (e0, s0) = plane(p[1], p[2]);
        let (e1, s1) = plane(p[2], p[0]);
        let (e2, s2) = plane(p[0], p[1]);
        let cx = x0 as f32 + 0.5;
        for y in y0..y1 {
            let cy = y as f32 + 0.5;
            let (mut w0, mut w1, mut w2) = (e0(cx, cy), e1(cx, cy), e2(cx, cy));
            let row = (y * self.w) as usize;
            for x in x0..x1 {
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    let z = w0 * d[0] + w1 * d[1] + w2 * d[2];
                    let i = row + x as usize;
                    if z > self.depth[i] {
                        self.depth[i] = z;
                        self.normal[i] =
                            flat.unwrap_or_else(|| pack_px(w0 * n[0] + w1 * n[1] + w2 * n[2]));
                    }
                }
                w0 += s0;
                w1 += s1;
                w2 += s2;
            }
        }
    }

    /// Covered pixels next to the background or next to a surface more
    /// than `step` farther away.
    fn edges(&self, step: f32) -> Vec<bool> {
        let (w, h) = (i64::from(self.w), i64::from(self.h));
        let mut marks = vec![false; self.depth.len()];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if !self.covered(i) {
                    continue;
                }
                let here = self.depth[i];
                let mut is_edge = false;
                'n: for r in 1..=EDGE_PX {
                    for (dx, dy) in [(r, 0), (-r, 0), (0, r), (0, -r)] {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let n = self.depth[(ny * w + nx) as usize];
                        if here - n > step * r as f32 {
                            is_edge = true;
                            break 'n;
                        }
                    }
                }
                marks[i] = is_edge;
            }
        }
        marks
    }

    /// Ambient occlusion of output pixel (`x`, `y`): how much nearby
    /// geometry rises above the tangent plane of the surface there, from
    /// 0 (open) to 1 (buried). Uses the first covered subsample.
    fn occlusion(&self, i: usize, frame: &Frame, kernel: &[[i32; 2]]) -> f32 {
        let (px, py) = ((i as u32 % self.w) as i32, (i as u32 / self.w) as i32);
        let n = unpack_px(self.normal[i]);
        let b = &frame.basis;
        // Depth change per supersampled pixel along x and y on the plane.
        let ne = n.dot(b.toward_eye).max(0.25);
        let per_px = 1.0 / frame.fit.scale.max(1e-9);
        let gx = -n.dot(b.right) / ne * per_px;
        let gy = n.dot(b.up) / ne * per_px;
        let here = self.depth[i];
        let radius = frame.ao_radius * per_px;
        let bias = 0.06 * radius;
        let mut occ = 0.0;
        for &[ox, oy] in kernel {
            let (sx, sy) = (px + ox, py + oy);
            if sx < 0 || sy < 0 || sx >= self.w as i32 || sy >= self.h as i32 {
                continue;
            }
            let j = (sy as u32 * self.w + sx as u32) as usize;
            if !self.covered(j) {
                continue;
            }
            let rise = self.depth[j] - (here + gx * ox as f32 + gy * oy as f32);
            if rise > bias {
                // Full weight up to one radius above, none past two, so a
                // part far in front casts no halo.
                occ += (2.0 - rise / radius).clamp(0.0, 1.0);
            }
        }
        occ / kernel.len() as f32
    }

    /// Averages each `SS`×`SS` block into one output pixel in linear
    /// light and darkens it by its (lightly blurred) ambient occlusion.
    ///
    /// A pixel inside one surface is shaded once, from its averaged
    /// normal; without smooth corner normals those are first blurred a
    /// little across facets (see [`smooth_interior`]). Pixels on an edge
    /// line, a silhouette or a crease shade each subsample.
    fn resolve(&self, frame: &Frame, width: u32, height: u32) -> Image {
        let edges = self.edges(frame.edge_step);
        // Sample offsets in whole supersampled pixels, per 2×2 position.
        let kernels: [[[i32; 2]; AO_SAMPLES]; 4] = std::array::from_fn(|k| {
            ao_kernel(ao_turn(k as u32 & 1, k as u32 >> 1))
                .map(|o| o.map(|v| (v * frame.ao_radius).round() as i32))
        });
        let crease = (printdex_mesh::CREASE_DEGREES.to_radians().cos() * 127.0 * 127.0) as i32;
        let light = &frame.lighting;
        let px = (width * height) as usize;
        // Per output pixel: linear colour (or the normal of an interior
        // pixel), covered subsamples, occlusion and the depth it was
        // measured at.
        // Interior pixels hold their summed normal in `color` until shaded.
        let mut color = vec![[0.0f32; 3]; px];
        let mut interior = vec![false; px];
        let mut cover = vec![0u32; px];
        let mut occ = vec![0.0f32; px];
        let mut depth = vec![f32::NEG_INFINITY; px];
        let mut last = ([0i8; 4], light.shade(Vec3::ZERO));
        for y in 0..height {
            for x in 0..width {
                let o = (y * width + x) as usize;
                let block: [usize; (SS * SS) as usize] = std::array::from_fn(|k| {
                    let (sx, sy) = (k as u32 % SS, k as u32 / SS);
                    ((y * SS + sy) * self.w + x * SS + sx) as usize
                });
                let n0 = self.normal[block[0]];
                let inside = block.iter().all(|&i| {
                    let n = self.normal[i];
                    let dot = i32::from(n[0]) * i32::from(n0[0])
                        + i32::from(n[1]) * i32::from(n0[1])
                        + i32::from(n[2]) * i32::from(n0[2]);
                    self.covered(i) && !edges[i] && dot >= crease
                });
                let mut first = None;
                if inside {
                    first = Some(block[0]);
                    cover[o] = SS * SS;
                    interior[o] = true;
                    let n: Vec3 = block.iter().map(|&i| unpack_px(self.normal[i])).sum();
                    color[o] = n.to_array();
                } else {
                    let mut sum = [0.0f32; 3];
                    for &i in &block {
                        if self.covered(i) {
                            first.get_or_insert(i);
                            let c = if edges[i] {
                                light.edge()
                            } else {
                                // Neighbours on a flat face share a
                                // normal: shade each normal once per run.
                                let n = self.normal[i];
                                if n != last.0 {
                                    last = (n, light.shade(unpack_px(n)));
                                }
                                last.1
                            };
                            for (s, c) in sum.iter_mut().zip(c) {
                                *s += c;
                            }
                            cover[o] += 1;
                        }
                    }
                    if cover[o] > 0 {
                        color[o] = sum.map(|s| s / cover[o] as f32);
                    }
                }
                if let Some(i) = first {
                    let kernel = &kernels[((x & 1) | ((y & 1) << 1)) as usize];
                    occ[o] = self.occlusion(i, frame, kernel);
                    depth[o] = self.depth[i];
                }
            }
        }
        if frame.blur_facets {
            smooth_interior(
                &mut color,
                &interior,
                &depth,
                width,
                height,
                frame.edge_step * SS as f32,
            );
        }
        // Shade interior pixels; a flat face repeats one normal.
        let mut last = ([f32::NAN; 3], [0.0f32; 3]);
        for (c, _) in color
            .iter_mut()
            .zip(&interior)
            .filter(|(_, inside)| **inside)
        {
            if *c != last.0 {
                last = (*c, light.shade(Vec3::from_array(*c).normalize_or_zero()));
            }
            *c = last.1;
        }
        // 3×3 blur of the occlusion over neighbours on the same surface.
        let near = frame.ao_radius / frame.fit.scale.max(1e-9);
        let srgb = srgb_table();
        let mut img = Image::transparent(width, height);
        for y in 0..height {
            for x in 0..width {
                let o = (y * width + x) as usize;
                if cover[o] == 0 {
                    continue;
                }
                let (mut total, mut n) = (0.0, 0.0);
                for ny in y.saturating_sub(1)..(y + 2).min(height) {
                    for nx in x.saturating_sub(1)..(x + 2).min(width) {
                        let j = (ny * width + nx) as usize;
                        if cover[j] > 0 && (depth[j] - depth[o]).abs() < near {
                            total += occ[j];
                            n += 1.0;
                        }
                    }
                }
                let c = light.occlude(color[o], total / n);
                let p = o * 4;
                for (dst, c) in img.rgba[p..p + 3].iter_mut().zip(c) {
                    *dst = srgb.get(c);
                }
                img.rgba[p + 3] = ((cover[o] * 255 + SS * SS / 2) / (SS * SS)) as u8;
            }
        }
        img
    }
}

/// Smooths flat-shaded facets in screen space: each interior pixel's
/// normal becomes the sum of the interior normals up to two pixels away
/// along x, then along y, that are on the same surface (no depth jump of
/// more than `step` per pixel) and within the crease angle of its own.
/// Facets of dense meshes are only a few pixels wide, so this hides them
/// for a fraction of the cost of welding the mesh; hard edges stay crisp.
fn smooth_interior(
    normals: &mut [[f32; 3]],
    interior: &[bool],
    depth: &[f32],
    width: u32,
    height: u32,
    step: f32,
) {
    const R: usize = 2;
    let crease = printdex_mesh::CREASE_DEGREES.to_radians().cos();
    let (w, h) = (width as usize, height as usize);
    let mut src = normals.to_vec();
    for (along, across, stride, next) in [(w, h, w, 1), (h, w, 1, w)] {
        src.copy_from_slice(normals);
        for l in 0..across {
            for k in 0..along {
                let i = l * stride + k * next;
                if !interior[i] {
                    continue;
                }
                let unit = Vec3::from_array(src[i]).normalize_or_zero();
                let mut sum = Vec3::ZERO;
                for t in k.saturating_sub(R)..(k + R + 1).min(along) {
                    let j = l * stride + t * next;
                    let n = Vec3::from_array(src[j]);
                    if interior[j]
                        && n.dot(unit) >= crease * n.length()
                        && (depth[j] - depth[i]).abs() <= step * t.abs_diff(k) as f32
                    {
                        sum += n;
                    }
                }
                normals[i] = sum.to_array();
            }
        }
    }
}

/// The shared linear-to-sRGB table.
fn srgb_table() -> &'static SrgbTable {
    static TABLE: std::sync::OnceLock<SrgbTable> = std::sync::OnceLock::new();
    TABLE.get_or_init(SrgbTable::new)
}

/// Framing and colours for one render, shared by the CPU and GPU paths.
pub(crate) struct Frame {
    /// Supersampled size.
    pub w: u32,
    pub h: u32,
    pub dmin: f32,
    pub dmax: f32,
    fit: Fit,
    basis: Basis,
    lighting: Lighting,
    edge_step: f32,
    /// Occlusion sample radius in supersampled pixels.
    ao_radius: f32,
    /// Blur interior normals in screen space: drawn flat, with facets a
    /// few pixels wide (denser meshes show real detail at pixel scale).
    blur_facets: bool,
}

/// One triangle ready to rasterize: corners in supersampled pixels,
/// depths (larger is nearer) and unit corner normals facing the eye.
pub(crate) struct Tri {
    pub px: [Vec2; 3],
    pub depth: [f32; 3],
    pub normal: [Vec3; 3],
}

/// Projects `mesh` (with smooth `normals`, one per corner, or else face
/// normals), calling `each` per triangle that is not degenerate. `None`
/// for an empty mesh.
pub(crate) fn prepare(
    mesh: &Mesh,
    normals: Option<&[PackedNormal]>,
    opts: &RenderOptions,
    mut each: impl FnMut(Tri),
) -> Option<Frame> {
    let (w, h) = (opts.width * SS, opts.height * SS);
    let basis = opts.camera.basis();
    let projected: Vec<(Vec2, f32)> = mesh
        .positions
        .iter()
        .map(|p| basis.project(Vec3::from_array(*p)))
        .collect();
    let first = projected.first()?;
    let (mut min, mut max) = (first.0, first.0);
    let (mut dmin, mut dmax) = (first.1, first.1);
    for (s, d) in &projected {
        min = min.min(*s);
        max = max.max(*s);
        dmin = dmin.min(*d);
        dmax = dmax.max(*d);
    }
    let fit = match opts.framing {
        Framing::OnBed(margins) => Fit::on_bed(min, max, w, h, margins),
        Framing::Orbit { zoom, pan } => {
            let sphere = Sphere::of(mesh)?;
            let (c, d) = basis.project(sphere.center);
            dmin = d - sphere.radius;
            dmax = d + sphere.radius;
            Fit::orbit(c, sphere.radius, w, h, zoom, pan)
        }
    };
    let normals = normals.map(|n| n.as_chunks::<3>().0);
    for (i, (tri, proj)) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(projected.as_chunks::<3>().0)
        .enumerate()
    {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(Vec3::from_array);
        let n = (b - a).cross(c - a);
        let len = n.length();
        if len == 0.0 || !len.is_finite() {
            continue;
        }
        // Two-sided: many files have inconsistent winding, and corner
        // normals follow their face's winding.
        let flip = if n.dot(basis.toward_eye) < 0.0 {
            -1.0
        } else {
            1.0
        };
        let normal = match normals.and_then(|n| n.get(i)) {
            Some(corners) => corners.map(|k| Vec3::from_array(unpack(k)) * flip),
            None => [n / len * flip; 3],
        };
        each(Tri {
            px: [proj[0].0, proj[1].0, proj[2].0].map(|s| fit.to_pixel(s)),
            depth: [proj[0].1, proj[1].1, proj[2].1],
            normal,
        });
    }
    Some(Frame {
        w,
        h,
        dmin,
        dmax,
        fit,
        basis,
        lighting: Lighting::new(&opts.material.ramp(), basis.toward_eye),
        edge_step: edge_step(fit.scale, dmin, dmax),
        ao_radius: AO_RADIUS * w.min(h) as f32,
        blur_facets: normals.is_none()
            && mesh.triangles() as u64 * BLUR_PIXELS_PER_TRIANGLE <= u64::from(w) * u64::from(h),
    })
}

/// A sphere around the whole mesh: the centre of its bounding box and
/// half its diagonal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub center: Vec3,
    pub radius: f32,
}

impl Sphere {
    pub fn of(mesh: &Mesh) -> Option<Sphere> {
        let b = mesh.bounds()?;
        let (min, max) = (Vec3::from_array(b.min), Vec3::from_array(b.max));
        Some(Sphere {
            center: (min + max) / 2.0,
            radius: ((max - min).length() / 2.0).max(1e-3),
        })
    }
}

/// Depth difference between neighbouring supersampled pixels that marks
/// an edge: a few pixels' worth, or 1.5% of the model's depth.
pub fn edge_step(scale: f32, dmin: f32, dmax: f32) -> f32 {
    let px_mm = if scale > 0.0 { 1.0 / scale } else { 0.0 };
    (4.0 * px_mm).max(0.015 * (dmax - dmin))
}

/// Shading, occlusion, edge lines and downsampling: the same for both
/// paths.
pub(crate) fn finish(target: Target, frame: &Frame, opts: &RenderOptions) -> Image {
    target.resolve(frame, opts.width, opts.height)
}

/// Supersampled size of a render.
pub(crate) fn target_size(opts: &RenderOptions) -> (u32, u32) {
    (opts.width * SS, opts.height * SS)
}

/// Whether smooth normals are worth computing for `mesh` in a picture of
/// `opts`'s size: only when triangles are big enough on screen for their
/// facets to show. Denser meshes average out in the supersampling, and
/// welding millions of corners would cost more than the render.
pub fn wants_smooth_normals(mesh: &Mesh, opts: &RenderOptions) -> bool {
    let (w, h) = target_size(opts);
    mesh.triangles() as u64 * SMOOTH_PIXELS_PER_TRIANGLE <= u64::from(w) * u64::from(h)
}

/// Supersampled pixels per triangle from which corners are welded for
/// smooth normals; welding costs about as much as rasterizing.
const SMOOTH_PIXELS_PER_TRIANGLE: u64 = 8;
/// Supersampled pixels per triangle from which flat facets are blurred
/// in screen space instead; below it they average out in supersampling.
const BLUR_PIXELS_PER_TRIANGLE: u64 = 1;

/// Renders `mesh` on the CPU, framed to fill the image. An empty mesh
/// gives a transparent image.
pub fn render(mesh: &Mesh, opts: &RenderOptions) -> Image {
    let normals = wants_smooth_normals(mesh, opts).then(|| printdex_mesh::corner_normals(mesh));
    render_with_normals(mesh, normals.as_deref(), opts)
}

/// [`render`] with smooth corner normals already computed (from
/// [`printdex_mesh::corner_normals`]; `None` draws faces flat), for
/// drawing one mesh many times.
pub fn render_with_normals(
    mesh: &Mesh,
    normals: Option<&[PackedNormal]>,
    opts: &RenderOptions,
) -> Image {
    let (w, h) = target_size(opts);
    let mut target = Target::new(w, h);
    let r = prepare(mesh, normals, opts, |t| {
        target.triangle(t.px, t.depth, t.normal)
    });
    match r {
        Some(frame) => finish(target, &frame, opts),
        None => Image::transparent(opts.width, opts.height),
    }
}

#[cfg(test)]
impl Frame {
    pub fn for_test(w: u32, h: u32, dmin: f32, dmax: f32) -> Frame {
        let basis = Camera::default().basis();
        Frame {
            w,
            h,
            dmin,
            dmax,
            fit: Fit::new(Vec2::ZERO, Vec2::ONE, w, h, 0.0),
            basis,
            lighting: Lighting::new(&Material::Unknown.ramp(), basis.toward_eye),
            edge_step: 1.0,
            ao_radius: 1.0,
            blur_facets: false,
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Axis-aligned box from `min` to `max`, 12 triangles.
    pub(crate) fn cuboid(min: [f32; 3], max: [f32; 3]) -> Mesh {
        let c = |i: usize| {
            [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ]
        };
        let faces = [
            [0, 1, 3, 2],
            [4, 6, 7, 5],
            [0, 4, 5, 1],
            [2, 3, 7, 6],
            [0, 2, 6, 4],
            [1, 5, 7, 3],
        ];
        let mut m = Mesh::default();
        for f in faces {
            m.positions.extend([c(f[0]), c(f[1]), c(f[2])]);
            m.positions.extend([c(f[0]), c(f[2]), c(f[3])]);
        }
        m
    }

    fn opaque(img: &Image) -> Vec<(u32, u32, [u8; 4])> {
        let mut out = Vec::new();
        for y in 0..img.height {
            for x in 0..img.width {
                let p = img.pixel(x, y);
                if p[3] > 0 {
                    out.push((x, y, p));
                }
            }
        }
        out
    }

    #[test]
    fn empty_mesh_renders_transparent() {
        let img = render(&Mesh::default(), &RenderOptions::default());
        assert_eq!((img.width, img.height), (384, 288));
        assert!(img.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn camera_frames_bounds() {
        let opts = RenderOptions::default();
        let m = Margins::THUMBNAIL;
        // Far from the origin and tiny: framing must still fill the image.
        let img = render(&cuboid([500.0, 500.0, 0.0], [501.0, 502.0, 3.0]), &opts);
        let px = opaque(&img);
        assert!(px.len() > 5000, "model should fill the frame");
        let (left, right) = (m.side * 384.0 - 1.0, 384.0 * (1.0 - m.side) + 1.0);
        let (top, bed) = (m.top * 288.0 - 1.0, 288.0 * (1.0 - m.bottom) + 1.0);
        for (x, y, _) in &px {
            assert!(*x as f32 >= left && (*x as f32) < right, "x {x}");
            assert!(*y as f32 >= top && (*y as f32) < bed, "y {y}");
        }
        let lowest = px.iter().map(|(_, y, _)| *y).max().unwrap() as f32;
        assert!(bed - lowest <= 3.0, "stands on the bed line ({lowest})");
    }

    #[test]
    fn cube_uses_three_ramp_tones() {
        let img = render(&cuboid([0.0; 3], [10.0; 3]), &RenderOptions::default());
        let r = Material::Unknown.ramp();
        let px = opaque(&img);
        let count = |c: [u8; 3]| {
            px.iter()
                .filter(|(_, _, p)| (0..3).all(|k| p[k].abs_diff(c[k]) <= 1))
                .count()
        };
        let total = px.len();
        for (name, tone) in [("top", r.top), ("side", r.side), ("shade", r.shade)] {
            assert!(count(tone) * 10 > total, "{name} tone covers > 10%");
        }
    }

    #[test]
    fn materials_tint_the_render() {
        let opts = RenderOptions {
            material: Material::Petg,
            ..RenderOptions::default()
        };
        let img = render(&cuboid([0.0; 3], [10.0; 3]), &opts);
        let side = Material::Petg.ramp().side;
        assert!(
            opaque(&img)
                .iter()
                .any(|(_, _, p)| (0..3).all(|k| p[k].abs_diff(side[k]) <= 1))
        );
    }

    #[test]
    fn depth_test_keeps_nearest() {
        let tri = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
        ];
        let mut a = Target::new(10, 10);
        a.triangle(tri, [1.0; 3], [Vec3::X; 3]);
        a.triangle(tri, [2.0; 3], [Vec3::Y; 3]);
        let mut b = Target::new(10, 10);
        b.triangle(tri, [2.0; 3], [Vec3::Y; 3]);
        b.triangle(tri, [1.0; 3], [Vec3::X; 3]);
        assert_eq!(a.normal[0], [0, 127, 0, 0]);
        assert_eq!(a.normal, b.normal);
    }

    #[test]
    fn curved_surfaces_shade_smoothly() {
        // A 24-sided cylinder: with smooth normals the side has many
        // tones blending into each other, not 12 flat bands.
        let mut m = Mesh::default();
        let seg = 24;
        for i in 0..seg {
            let a = |k: u32| {
                let t = k as f32 / seg as f32 * std::f32::consts::TAU;
                (10.0 * t.cos(), 10.0 * t.sin())
            };
            let ((x0, y0), (x1, y1)) = (a(i), a(i + 1));
            m.positions
                .extend([[x0, y0, 0.0], [x1, y1, 0.0], [x1, y1, 20.0]]);
            m.positions
                .extend([[x0, y0, 0.0], [x1, y1, 20.0], [x0, y0, 20.0]]);
        }
        let img = render(&m, &RenderOptions::default());
        let row = img.height / 2;
        let mut tones = std::collections::HashSet::new();
        for x in 0..img.width {
            let p = img.pixel(x, row);
            if p[3] == 255 {
                tones.insert(p);
            }
        }
        assert!(tones.len() > 40, "{} tones across the side", tones.len());
    }

    #[test]
    fn crevices_are_darker_than_open_faces() {
        // An L-shaped bracket: the floor next to the wall is occluded.
        let mut m = cuboid([0.0; 3], [40.0, 40.0, 4.0]);
        m.positions
            .extend(cuboid([0.0, 30.0, 4.0], [40.0, 40.0, 40.0]).positions);
        let opts = RenderOptions::default();
        let img = render(&m, &opts);
        let basis = opts.camera.basis();
        // Pixels of floor points near and far from the wall, located via
        // the depth target of a second render.
        let (w, h) = target_size(&opts);
        let mut t = Target::new(w, h);
        let frame = prepare(&m, None, &opts, |tri| {
            t.triangle(tri.px, tri.depth, tri.normal)
        })
        .unwrap();
        let at = |p: Vec3| {
            let px = frame.fit.to_pixel(basis.project(p).0) / SS as f32;
            img.pixel(px.x as u32, px.y as u32)
        };
        let open = at(Vec3::new(20.0, 5.0, 4.0));
        let corner = at(Vec3::new(20.0, 29.5, 4.0));
        let lum = |p: [u8; 4]| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]);
        assert!(
            lum(corner) + 20 < lum(open),
            "corner {corner:?} vs open {open:?}"
        );
    }

    #[test]
    fn edge_line_drawn_on_silhouette() {
        let img = render(&cuboid([0.0; 3], [10.0; 3]), &RenderOptions::default());
        let e = Material::Unknown.ramp().edge;
        let near = |p: [u8; 4]| (0..3).all(|k| (i32::from(p[k]) - i32::from(e[k])).abs() < 24);
        let px = opaque(&img);
        let edge = px.iter().filter(|(_, _, p)| near(*p)).count();
        assert!(edge > 200, "outline present ({edge} px)");
        // Interior pixels are not edge-coloured: the centre is a face tone.
        let c = img.pixel(192, 144);
        assert!(c[3] == 255 && !near(c));
    }

    #[test]
    fn render_is_deterministic() {
        let m = cuboid([0.0; 3], [10.0, 20.0, 5.0]);
        let o = RenderOptions::default();
        assert_eq!(render(&m, &o), render(&m, &o));
    }
}
