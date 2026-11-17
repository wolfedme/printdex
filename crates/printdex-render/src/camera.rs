//! Orbit camera shared by thumbnails and the interactive viewer.
//!
//! World space is the printer's: millimetres, Z up, the front of the bed
//! towards -Y. The camera orbits a target; projection is orthographic so
//! proportions read true.

use glam::{Vec2, Vec3};

/// Default orbit: from the front right, 30° above the bed.
pub const DEFAULT_AZIMUTH: f32 = 30.0_f32.to_radians();
pub const DEFAULT_ELEVATION: f32 = 30.0_f32.to_radians();
/// The camera never goes past straight above or straight below.
pub const MAX_ELEVATION: f32 = 89.0_f32.to_radians();

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// Around Z, from the front (-Y) towards the right (+X), radians.
    pub azimuth: f32,
    /// Above the bed plane, radians.
    pub elevation: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Camera {
            azimuth: DEFAULT_AZIMUTH,
            elevation: DEFAULT_ELEVATION,
        }
    }
}

/// Orthonormal view basis: screen right, screen up, and towards the eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Basis {
    pub right: Vec3,
    pub up: Vec3,
    pub toward_eye: Vec3,
}

impl Basis {
    /// Screen position (x right, y up) and depth (larger is nearer).
    pub fn project(&self, p: Vec3) -> (Vec2, f32) {
        (
            Vec2::new(p.dot(self.right), p.dot(self.up)),
            p.dot(self.toward_eye),
        )
    }
}

impl Camera {
    /// Unit vector from the target towards the eye.
    pub fn toward_eye(&self) -> Vec3 {
        let el = self.elevation.clamp(-MAX_ELEVATION, MAX_ELEVATION);
        Vec3::new(
            self.azimuth.sin() * el.cos(),
            -self.azimuth.cos() * el.cos(),
            el.sin(),
        )
    }

    pub fn basis(&self) -> Basis {
        let toward_eye = self.toward_eye();
        let right = Vec3::Z.cross(toward_eye).normalize();
        let up = toward_eye.cross(right);
        Basis {
            right,
            up,
            toward_eye,
        }
    }

    /// Turns the camera by `d_azimuth` and `d_elevation` radians, keeping
    /// the elevation within ±[`MAX_ELEVATION`].
    pub fn orbit(&mut self, d_azimuth: f32, d_elevation: f32) {
        self.azimuth = (self.azimuth + d_azimuth).rem_euclid(std::f32::consts::TAU);
        self.elevation = (self.elevation + d_elevation).clamp(-MAX_ELEVATION, MAX_ELEVATION);
    }
}

/// Maps projected screen coordinates into a `width` × `height` pixel
/// image (y down), fitting `min..max` with `margin` (a fraction of the
/// smaller side) on every side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub scale: f32,
    pub offset: Vec2,
    pub height: f32,
}

impl Fit {
    pub fn new(min: Vec2, max: Vec2, width: u32, height: u32, margin: f32) -> Fit {
        let (w, h) = (width as f32, height as f32);
        let pad = margin * w.min(h);
        let size = (max - min).max(Vec2::splat(1e-6));
        let scale = ((w - 2.0 * pad) / size.x)
            .min((h - 2.0 * pad) / size.y)
            .max(0.0);
        let center = (min + max) / 2.0;
        Fit {
            scale,
            offset: Vec2::new(w / 2.0, h / 2.0) - center * scale * Vec2::new(1.0, -1.0),
            height: h,
        }
    }

    /// Fits `min..max` so the model stands on an imaginary bed: centred
    /// horizontally, its lowest point `margins.bottom` of the height above
    /// the image's bottom edge, scaled as large as the margins allow. The
    /// app draws its contact shadow at that line.
    pub fn on_bed(min: Vec2, max: Vec2, width: u32, height: u32, margins: Margins) -> Fit {
        let (w, h) = (width as f32, height as f32);
        let size = (max - min).max(Vec2::splat(1e-6));
        let scale = ((w * (1.0 - 2.0 * margins.side)) / size.x)
            .min((h * (1.0 - margins.top - margins.bottom)) / size.y)
            .max(0.0);
        let cx = (min.x + max.x) / 2.0;
        Fit {
            scale,
            offset: Vec2::new(
                w / 2.0 - cx * scale,
                h * (1.0 - margins.bottom) + min.y * scale,
            ),
            height: h,
        }
    }

    /// Pixel position of screen point `s`.
    pub fn to_pixel(&self, s: Vec2) -> Vec2 {
        Vec2::new(s.x * self.scale, -s.y * self.scale) + self.offset
    }
}

/// Empty space around a model, as fractions of the image width (sides)
/// and height (top, bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Margins {
    pub side: f32,
    pub top: f32,
    pub bottom: f32,
}

impl Margins {
    /// Thumbnails: the model's lowest point sits on the line where the
    /// app's contact shadow is drawn (80% down, like the placeholder).
    pub const THUMBNAIL: Margins = Margins {
        side: 0.06,
        top: 0.06,
        bottom: 0.2,
    };
}

/// How a render places the model in the image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Framing {
    /// Thumbnails: scaled to fill the image, standing on the bed line.
    OnBed(Margins),
    /// The interactive viewer: framed by the model's bounding sphere, so
    /// the scale holds still while the camera orbits.
    Orbit { zoom: f32, pan: Vec2 },
}

/// Space around the bounding sphere in the viewer, a fraction of the
/// smaller side.
pub const ORBIT_MARGIN: f32 = 0.04;
/// Zoom limits of the viewer, relative to the whole model in view.
pub const MIN_ZOOM: f32 = 0.25;
pub const MAX_ZOOM: f32 = 40.0;
/// Orbit speed, radians per pixel dragged.
pub const ORBIT_PER_PX: f32 = 0.008;
/// Zoom factor per wheel line.
pub const ZOOM_PER_LINE: f32 = 1.12;

/// The viewer's camera: orbit angles, zoom and pan.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub camera: Camera,
    pub zoom: f32,
    /// Offset of the model's centre from the image centre, as a fraction
    /// of the image's smaller side (x right, y down).
    pub pan: Vec2,
}

impl Default for View {
    fn default() -> Self {
        View {
            camera: Camera::default(),
            zoom: 1.0,
            pan: Vec2::ZERO,
        }
    }
}

impl View {
    /// Orbits for a drag of `dx`, `dy` pixels: right turns the model to
    /// the right, down tilts its top towards the viewer.
    pub fn orbit_px(&mut self, dx: f32, dy: f32) {
        self.camera.orbit(-dx * ORBIT_PER_PX, dy * ORBIT_PER_PX);
    }

    /// Pans for a drag of `dx`, `dy` pixels in a viewport whose smaller
    /// side is `min_side` pixels; the model follows the pointer.
    pub fn pan_px(&mut self, dx: f32, dy: f32, min_side: f32) {
        if min_side > 0.0 {
            self.pan += Vec2::new(dx, dy) / min_side;
        }
    }

    /// Zooms by `lines` wheel lines (positive is closer), about the image
    /// centre, within [`MIN_ZOOM`]..=[`MAX_ZOOM`].
    pub fn zoom_lines(&mut self, lines: f32) {
        let next = (self.zoom * ZOOM_PER_LINE.powf(lines)).clamp(MIN_ZOOM, MAX_ZOOM);
        // Keep what is under the image centre there.
        self.pan *= next / self.zoom;
        self.zoom = next;
    }

    pub fn framing(&self) -> Framing {
        Framing::Orbit {
            zoom: self.zoom,
            pan: self.pan,
        }
    }
}

impl Fit {
    /// Viewer framing: the sphere of `radius` around the screen point
    /// `center` fits the smaller side at zoom 1, and sits at the image
    /// centre moved by `pan`.
    pub fn orbit(center: Vec2, radius: f32, width: u32, height: u32, zoom: f32, pan: Vec2) -> Fit {
        let (w, h) = (width as f32, height as f32);
        let side = w.min(h);
        let scale = (side * (1.0 - 2.0 * ORBIT_MARGIN) / (2.0 * radius.max(1e-6)) * zoom).max(0.0);
        let at = Vec2::new(w / 2.0, h / 2.0) + pan * side;
        Fit {
            scale,
            offset: at - Vec2::new(center.x * scale, -center.y * scale),
            height: h,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_sees_front_right_and_top() {
        let e = Camera::default().toward_eye();
        assert!(e.x > 0.0 && e.y < 0.0 && e.z > 0.0);
        assert!((e.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn basis_is_orthonormal_with_z_up_on_screen() {
        let b = Camera::default().basis();
        assert!(b.right.dot(b.up).abs() < 1e-5);
        assert!(b.right.dot(b.toward_eye).abs() < 1e-5);
        assert!(b.up.z > 0.0);
        assert!(b.right.z.abs() < 1e-6);
    }

    #[test]
    fn orbit_clamps_elevation() {
        let mut c = Camera::default();
        c.orbit(0.0, 10.0);
        assert_eq!(c.elevation, MAX_ELEVATION);
        c.orbit(0.0, -20.0);
        assert_eq!(c.elevation, -MAX_ELEVATION);
        c.orbit(std::f32::consts::TAU, 0.0);
        assert!((c.azimuth - DEFAULT_AZIMUTH).abs() < 1e-5);
    }

    #[test]
    fn on_bed_puts_lowest_point_on_the_bed_line() {
        let m = Margins::THUMBNAIL;
        // Wide and flat: width-limited, so it sits low with room above.
        let f = Fit::on_bed(Vec2::new(0.0, 0.0), Vec2::new(100.0, 10.0), 400, 300, m);
        assert!((f.to_pixel(Vec2::new(50.0, 0.0)).y - 240.0).abs() < 1e-3);
        assert!((f.to_pixel(Vec2::new(0.0, 0.0)).x - 24.0).abs() < 1e-3);
        assert!(f.to_pixel(Vec2::new(0.0, 10.0)).y > 100.0);
        // Tall: height-limited, top at the top margin, centred.
        let f = Fit::on_bed(Vec2::new(-5.0, 2.0), Vec2::new(5.0, 102.0), 400, 300, m);
        assert!((f.to_pixel(Vec2::new(0.0, 102.0)).y - 18.0).abs() < 1e-3);
        assert!((f.to_pixel(Vec2::new(0.0, 2.0)).y - 240.0).abs() < 1e-3);
        assert!((f.to_pixel(Vec2::new(0.0, 50.0)).x - 200.0).abs() < 1e-3);
    }

    #[test]
    fn zoom_limits() {
        let mut v = View::default();
        v.zoom_lines(1000.0);
        assert_eq!(v.zoom, MAX_ZOOM);
        v.zoom_lines(-1000.0);
        assert_eq!(v.zoom, MIN_ZOOM);
    }

    #[test]
    fn zoom_keeps_the_centre_point() {
        let mut v = View::default();
        v.pan_px(100.0, -50.0, 500.0);
        let f = Fit::orbit(Vec2::ZERO, 10.0, 500, 500, v.zoom, v.pan);
        // Model point under the image centre before zooming.
        let s = (Vec2::splat(250.0) - f.offset) / Vec2::new(f.scale, -f.scale);
        v.zoom_lines(3.0);
        let g = Fit::orbit(Vec2::ZERO, 10.0, 500, 500, v.zoom, v.pan);
        let p = g.to_pixel(s);
        assert!((p - Vec2::splat(250.0)).length() < 1e-2, "{p}");
    }

    #[test]
    fn pan_moves_target() {
        let mut v = View::default();
        let before = Fit::orbit(Vec2::ZERO, 10.0, 400, 300, v.zoom, v.pan).to_pixel(Vec2::ZERO);
        v.pan_px(30.0, 20.0, 300.0);
        let after = Fit::orbit(Vec2::ZERO, 10.0, 400, 300, v.zoom, v.pan).to_pixel(Vec2::ZERO);
        assert!((after - before - Vec2::new(30.0, 20.0)).length() < 1e-3);
    }

    #[test]
    fn orbit_fit_holds_scale_while_turning() {
        let a = Fit::orbit(Vec2::new(3.0, 4.0), 10.0, 400, 300, 1.0, Vec2::ZERO);
        assert!((a.to_pixel(Vec2::new(3.0, 4.0)) - Vec2::new(200.0, 150.0)).length() < 1e-3);
        assert!((a.scale - 300.0 * (1.0 - 2.0 * ORBIT_MARGIN) / 20.0).abs() < 1e-4);
    }

    #[test]
    fn fit_centres_and_keeps_margin() {
        let f = Fit::new(Vec2::new(-10.0, -5.0), Vec2::new(10.0, 5.0), 400, 300, 0.1);
        let a = f.to_pixel(Vec2::new(-10.0, 5.0));
        let b = f.to_pixel(Vec2::new(10.0, -5.0));
        assert!((a.x - 30.0).abs() < 1e-3, "{a}");
        assert!((b.x - 370.0).abs() < 1e-3, "{b}");
        assert!(((a.y + b.y) / 2.0 - 150.0).abs() < 1e-3);
        assert!(a.y < b.y, "screen up is image up");
    }
}
