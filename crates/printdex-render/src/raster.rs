//! CPU rasterizer for thumbnails: flat-shaded triangles in the material
//! ramp with a z-buffer, 2×2 supersampling and an edge line where the
//! model meets the background or a farther surface.
//!
//! No GPU is needed, so thumbnails render the same on every machine,
//! including CI runners and remote desktops without a capable GPU.

use glam::{Vec2, Vec3};
use printdex_mesh::Mesh;

use crate::camera::{Camera, Fit, Margins};
use crate::material::{Material, Rgb, shade};

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
    /// Empty space around the model; its lowest point sits on the bottom
    /// margin line.
    pub margins: Margins,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            width: 384,
            height: 288,
            material: Material::Unknown,
            camera: Camera::default(),
            margins: Margins::THUMBNAIL,
        }
    }
}

/// Supersampled colour and depth buffers.
pub(crate) struct Target {
    pub w: u32,
    pub h: u32,
    pub depth: Vec<f32>,
    pub color: Vec<Rgb>,
}

impl Target {
    pub fn new(w: u32, h: u32) -> Target {
        let n = (w * h) as usize;
        Target {
            w,
            h,
            depth: vec![f32::NEG_INFINITY; n],
            color: vec![[0; 3]; n],
        }
    }

    fn covered(&self, i: usize) -> bool {
        self.depth[i] > f32::NEG_INFINITY
    }

    /// Fills the triangle with pixel corners `p` and depths `d` (larger is
    /// nearer) where it is nearer than what is there.
    pub fn triangle(&mut self, p: [Vec2; 3], d: [f32; 3], color: Rgb) {
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
        for y in y0..y1 {
            for x in x0..x1 {
                let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = edge(p[1], p[2], c) * inv;
                let w1 = edge(p[2], p[0], c) * inv;
                let w2 = edge(p[0], p[1], c) * inv;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * d[0] + w1 * d[1] + w2 * d[2];
                let i = (y * self.w + x) as usize;
                if z > self.depth[i] {
                    self.depth[i] = z;
                    self.color[i] = color;
                }
            }
        }
    }

    /// Paints `edge` on covered pixels next to the background or next to a
    /// surface more than `step` farther away.
    pub fn edges(&mut self, edge: Rgb, step: f32) {
        let (w, h) = (i64::from(self.w), i64::from(self.h));
        let mut marks = Vec::new();
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
                if is_edge {
                    marks.push(i);
                }
            }
        }
        for i in marks {
            self.color[i] = edge;
        }
    }

    /// Averages each `SS`×`SS` block into one output pixel.
    pub fn resolve(&self, width: u32, height: u32) -> Image {
        let mut img = Image::transparent(width, height);
        for y in 0..height {
            for x in 0..width {
                let mut sum = [0u32; 3];
                let mut n = 0u32;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let i = ((y * SS + sy) * self.w + x * SS + sx) as usize;
                        if self.covered(i) {
                            for (s, c) in sum.iter_mut().zip(self.color[i]) {
                                *s += u32::from(c);
                            }
                            n += 1;
                        }
                    }
                }
                if let Some(nz) = std::num::NonZeroU32::new(n) {
                    let o = ((y * width + x) * 4) as usize;
                    for (dst, s) in img.rgba[o..o + 3].iter_mut().zip(sum) {
                        *dst = ((s + n / 2) / nz) as u8;
                    }
                    img.rgba[o + 3] = ((n * 255 + SS * SS / 2) / (SS * SS)) as u8;
                }
            }
        }
        img
    }
}

/// Framing and colours for one render, shared by the CPU and GPU paths.
pub(crate) struct Frame {
    /// Supersampled size.
    pub w: u32,
    pub h: u32,
    pub dmin: f32,
    pub dmax: f32,
    fit: Fit,
    edge: Rgb,
}

/// One triangle ready to rasterize: corners in supersampled pixels,
/// depths (larger is nearer) and its flat colour.
pub(crate) struct Tri {
    pub px: [Vec2; 3],
    pub depth: [f32; 3],
    pub color: Rgb,
}

/// Projects and shades `mesh`, calling `each` per visible-able triangle.
/// `None` for an empty mesh.
pub(crate) fn prepare(
    mesh: &Mesh,
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
    let fit = Fit::on_bed(min, max, w, h, opts.margins);
    let ramp = opts.material.ramp();
    for (tri, proj) in mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .zip(projected.as_chunks::<3>().0)
    {
        let [a, b, c] = [tri[0], tri[1], tri[2]].map(Vec3::from_array);
        let n = (b - a).cross(c - a);
        let len = n.length();
        if len == 0.0 || !len.is_finite() {
            continue;
        }
        let mut n = n / len;
        // Two-sided: many files have inconsistent winding.
        if n.dot(basis.toward_eye) < 0.0 {
            n = -n;
        }
        each(Tri {
            px: [proj[0].0, proj[1].0, proj[2].0].map(|s| fit.to_pixel(s)),
            depth: [proj[0].1, proj[1].1, proj[2].1],
            color: shade(&ramp, n),
        });
    }
    Some(Frame {
        w,
        h,
        dmin,
        dmax,
        fit,
        edge: ramp.edge,
    })
}

/// Edge lines and downsampling: the same for both paths.
pub(crate) fn finish(mut target: Target, frame: &Frame, opts: &RenderOptions) -> Image {
    // A depth jump of more than a few pixels' worth (or 1.5% of the
    // model's depth) between neighbours marks an edge.
    let px_mm = if frame.fit.scale > 0.0 {
        1.0 / frame.fit.scale
    } else {
        0.0
    };
    target.edges(
        frame.edge,
        (4.0 * px_mm).max(0.015 * (frame.dmax - frame.dmin)),
    );
    target.resolve(opts.width, opts.height)
}

/// Supersampled size of a render.
pub(crate) fn target_size(opts: &RenderOptions) -> (u32, u32) {
    (opts.width * SS, opts.height * SS)
}

/// Renders `mesh` on the CPU, framed to fill the image. An empty mesh
/// gives a transparent image.
pub fn render(mesh: &Mesh, opts: &RenderOptions) -> Image {
    let (w, h) = target_size(opts);
    let mut target = Target::new(w, h);
    match prepare(mesh, opts, |t| target.triangle(t.px, t.depth, t.color)) {
        Some(frame) => finish(target, &frame, opts),
        None => Image::transparent(opts.width, opts.height),
    }
}

#[cfg(test)]
impl Frame {
    pub fn for_test(w: u32, h: u32, dmin: f32, dmax: f32) -> Frame {
        Frame {
            w,
            h,
            dmin,
            dmax,
            fit: Fit::new(Vec2::ZERO, Vec2::ONE, w, h, 0.0),
            edge: [0; 3],
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
        let count = |c: [u8; 3]| px.iter().filter(|(_, _, p)| p[..3] == c).count();
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
        assert!(opaque(&img).iter().any(|(_, _, p)| p[..3] == side));
    }

    #[test]
    fn depth_test_keeps_nearest() {
        let tri = [
            Vec2::new(0.0, 0.0),
            Vec2::new(10.0, 0.0),
            Vec2::new(0.0, 10.0),
        ];
        let mut a = Target::new(10, 10);
        a.triangle(tri, [1.0; 3], [1, 1, 1]);
        a.triangle(tri, [2.0; 3], [2, 2, 2]);
        let mut b = Target::new(10, 10);
        b.triangle(tri, [2.0; 3], [2, 2, 2]);
        b.triangle(tri, [1.0; 3], [1, 1, 1]);
        assert_eq!(a.color[0], [2, 2, 2]);
        assert_eq!(a.color, b.color);
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
