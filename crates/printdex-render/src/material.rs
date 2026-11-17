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
}
