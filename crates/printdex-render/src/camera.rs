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

    /// Pixel position of screen point `s`.
    pub fn to_pixel(&self, s: Vec2) -> Vec2 {
        Vec2::new(s.x * self.scale, -s.y * self.scale) + self.offset
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
