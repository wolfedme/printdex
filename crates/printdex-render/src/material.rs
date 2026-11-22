//! Material colour ramps (DESIGN.md "Secondary (material colours)") and
//! the shading that maps a surface direction onto a ramp.
//!
//! Material colours are fixed in every colour theme. Previews are opaque.

use glam::Vec3;

/// An sRGB colour, 8 bits per channel.
pub type Rgb = [u8; 3];

/// What a model is printed in. Unknown uses the ASA neutral ramp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Material {
    Petg,
    Pla,
    Tpu,
    #[default]
    Unknown,
}

/// The four tones of a material: top face, side (the swatch), shade, edge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ramp {
    pub top: Rgb,
    pub side: Rgb,
    pub shade: Rgb,
    pub edge: Rgb,
}

const fn hex(v: u32) -> Rgb {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

impl Material {
    pub fn ramp(self) -> Ramp {
        match self {
            Material::Petg => Ramp {
                top: hex(0x90e2e1),
                side: hex(0x2bb3b9),
                shade: hex(0x008693),
                edge: hex(0x006d78),
            },
            Material::Pla => Ramp {
                top: hex(0xffca91),
                side: hex(0xf49752),
                shade: hex(0xd16631),
                edge: hex(0xae5528),
            },
            Material::Tpu => Ramp {
                top: hex(0xf6b7e0),
                side: hex(0xde75b4),
                shade: hex(0xb54481),
                edge: hex(0x943569),
            },
            Material::Unknown => Ramp {
                top: hex(0xc0c4cb),
                side: hex(0x81868f),
                shade: hex(0x4f5661),
                edge: hex(0x3c434d),
            },
        }
    }
}

/// Key light, world space (Z up, viewer at the front right): from above,
/// slightly front and left, so a box shows its top in the top tone, its
/// front in the side tone and its right side in the shade tone, like the
/// placeholder in the design.
pub fn light_dir() -> Vec3 {
    Vec3::new(-0.35, -0.6, 1.0).normalize()
}

/// Light term where each tone holds flat, so planar faces get exactly one
/// ramp colour and curved surfaces blend between them.
const SHADE_UNTIL: f32 = 0.0;
const SIDE_FROM: f32 = 0.4;
const SIDE_UNTIL: f32 = 0.6;
const TOP_FROM: f32 = 0.78;

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let ch = |i: usize| (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t).round() as u8;
    [ch(0), ch(1), ch(2)]
}

/// Colour of a surface with unit `normal` (world space) in `ramp`.
pub fn shade(ramp: &Ramp, normal: Vec3) -> Rgb {
    let t = normal.dot(light_dir());
    if t <= SHADE_UNTIL {
        ramp.shade
    } else if t < SIDE_FROM {
        mix(
            ramp.shade,
            ramp.side,
            (t - SHADE_UNTIL) / (SIDE_FROM - SHADE_UNTIL),
        )
    } else if t <= SIDE_UNTIL {
        ramp.side
    } else if t < TOP_FROM {
        mix(
            ramp.side,
            ramp.top,
            (t - SIDE_UNTIL) / (TOP_FROM - SIDE_UNTIL),
        )
    } else {
        ramp.top
    }
}

/// Where the ramp's tones hold flat and blend (for the GPU shaders).
pub(crate) fn ramp_breaks() -> [f32; 4] {
    [SHADE_UNTIL, SIDE_FROM, SIDE_UNTIL, TOP_FROM]
}

/// Strength of the soft plastic highlight, added in linear light.
pub const SPECULAR: f32 = 0.14;
/// Ambient occlusion: sample radius as a share of the image's smaller
/// side, and how far a fully occluded point moves towards the edge tone
/// (so crevices deepen in the material's own hue rather than greying).
pub const AO_RADIUS: f32 = 0.03;
pub const AO_STRENGTH: f32 = 0.8;
/// Samples per pixel of the occlusion pass.
pub const AO_SAMPLES: usize = 8;

/// An sRGB channel (0..=255) in linear light.
pub fn to_linear(c: u8) -> f32 {
    let c = f32::from(c) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// A linear channel as sRGB 0..=255.
pub fn to_srgb(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    s * 255.0
}

/// Linear light to 8-bit sRGB through a table: exact to within one level,
/// and fast enough to run per pixel.
pub(crate) struct SrgbTable(Vec<u8>);

impl SrgbTable {
    const STEPS: usize = 4096;

    pub fn new() -> SrgbTable {
        SrgbTable(
            (0..Self::STEPS)
                .map(|i| to_srgb(i as f32 / (Self::STEPS - 1) as f32).round() as u8)
                .collect(),
        )
    }

    pub fn get(&self, c: f32) -> u8 {
        let i = (c.clamp(0.0, 1.0) * (Self::STEPS - 1) as f32 + 0.5) as usize;
        self.0[i.min(Self::STEPS - 1)]
    }
}

/// How a model is lit for one picture: the ramp in linear light, the key
/// light and the highlight direction (which depends on the camera).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lighting {
    /// Top, side, shade and edge tones, linear.
    pub tones: [[f32; 3]; 4],
    pub light: Vec3,
    /// Halfway between the light and the eye.
    pub half: Vec3,
    /// Colour of the highlight: the top tone halfway to white.
    pub sheen: [f32; 3],
}

impl Lighting {
    pub fn new(ramp: &Ramp, toward_eye: Vec3) -> Lighting {
        let lin = |c: Rgb| c.map(to_linear);
        let top = lin(ramp.top);
        Lighting {
            sheen: top.map(|c| (c + 1.0) / 2.0),
            tones: [
                lin(ramp.top),
                lin(ramp.side),
                lin(ramp.shade),
                lin(ramp.edge),
            ],
            light: light_dir(),
            half: (light_dir() + toward_eye)
                .try_normalize()
                .unwrap_or(Vec3::Z),
        }
    }

    pub fn edge(&self) -> [f32; 3] {
        self.tones[3]
    }

    /// Linear colour of a surface with unit `normal` (facing the eye):
    /// the ramp tone for its light term (as [`shade`]) plus a soft
    /// highlight that only curved surfaces catch.
    pub fn shade(&self, normal: Vec3) -> [f32; 3] {
        let [top, side, shade, _] = self.tones;
        let t = normal.dot(self.light);
        let base = if t <= SHADE_UNTIL {
            shade
        } else if t < SIDE_FROM {
            mix3(shade, side, (t - SHADE_UNTIL) / (SIDE_FROM - SHADE_UNTIL))
        } else if t <= SIDE_UNTIL {
            side
        } else if t < TOP_FROM {
            mix3(side, top, (t - SIDE_UNTIL) / (TOP_FROM - SIDE_UNTIL))
        } else {
            top
        };
        let s = specular(normal.dot(self.half));
        [0, 1, 2].map(|i| base[i] + s * self.sheen[i])
    }

    /// `color` darkened for ambient occlusion `occ` (0 open, 1 buried).
    pub fn occlude(&self, color: [f32; 3], occ: f32) -> [f32; 3] {
        mix3(color, self.edge(), occ * AO_STRENGTH)
    }
}

/// The highlight for `n·h`: a soft lobe (exponent 16, by squaring)
/// that flat faces of a box at the default views still miss.
pub fn specular(n_dot_h: f32) -> f32 {
    let mut x = n_dot_h.max(0.0);
    for _ in 0..4 {
        x *= x;
    }
    SPECULAR * x
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| a[i] + (b[i] - a[i]) * t)
}

/// Offsets of the occlusion samples on a unit disc: a golden-angle
/// spiral turned by `turn` (a fraction of a full turn). Neighbouring
/// pixels use different turns (see [`ao_turn`]) and a small blur merges
/// them, so the pattern never shows.
pub fn ao_kernel(turn: f32) -> [[f32; 2]; AO_SAMPLES] {
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    std::array::from_fn(|i| {
        let r = ((i as f32 + 0.5) / AO_SAMPLES as f32).sqrt();
        let a = i as f32 * golden + turn * std::f32::consts::TAU;
        [r * a.cos(), r * a.sin()]
    })
}

/// Kernel turn of output pixel (`x`, `y`): four turns in a 2×2 pattern.
pub fn ao_turn(x: u32, y: u32) -> f32 {
    [0.0, 0.5, 0.75, 0.25][((x & 1) | ((y & 1) << 1)) as usize] / AO_SAMPLES as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramps_match_design_md() {
        let r = Material::Unknown.ramp();
        assert_eq!(r.top, [0xc0, 0xc4, 0xcb]);
        assert_eq!(r.side, [0x81, 0x86, 0x8f]);
        assert_eq!(r.shade, [0x4f, 0x56, 0x61]);
        assert_eq!(r.edge, [0x3c, 0x43, 0x4d]);
        assert_eq!(Material::Petg.ramp().side, [0x2b, 0xb3, 0xb9]);
        assert_eq!(Material::Pla.ramp().side, [0xf4, 0x97, 0x52]);
        assert_eq!(Material::Tpu.ramp().side, [0xde, 0x75, 0xb4]);
    }

    #[test]
    fn box_faces_get_one_tone_each() {
        let r = Material::Unknown.ramp();
        assert_eq!(shade(&r, Vec3::Z), r.top);
        assert_eq!(shade(&r, Vec3::NEG_Y), r.side);
        assert_eq!(shade(&r, Vec3::X), r.shade);
        assert_eq!(shade(&r, Vec3::NEG_Z), r.shade);
    }

    #[test]
    fn lighting_keeps_box_faces_on_the_ramp() {
        let r = Material::Pla.ramp();
        let eye = crate::Camera::default().toward_eye();
        let l = Lighting::new(&r, eye);
        let t = SrgbTable::new();
        let srgb = |c: [f32; 3]| c.map(|v| t.get(v));
        assert_eq!(srgb(l.shade(Vec3::Z)), r.top);
        assert_eq!(srgb(l.shade(Vec3::NEG_Y)), r.side);
        assert_eq!(srgb(l.shade(Vec3::X)), r.shade);
        // Facing the highlight direction: brighter than the top tone.
        let hi = srgb(l.shade(l.half));
        assert!(hi[2] > r.top[2], "{hi:?}");
    }

    #[test]
    fn srgb_round_trips() {
        let t = SrgbTable::new();
        for v in 0..=255u8 {
            assert_eq!(t.get(to_linear(v)), v);
        }
    }
}
