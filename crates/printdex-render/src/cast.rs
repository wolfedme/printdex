//! Float-to-integer conversions that saturate: NaN gives 0, values below
//! or above the target type give its minimum or maximum, and fractions
//! are truncated towards zero. That is what Rust's `as` does for floats;
//! these name the intent in one place.

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
)]
pub(crate) fn to_u8(v: f32) -> u8 {
    v as u8
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
)]
pub(crate) fn to_i8(v: f32) -> i8 {
    v as i8
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
)]
pub(crate) fn to_u32(v: f32) -> u32 {
    v as u32
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
)]
pub(crate) fn to_i32(v: f32) -> i32 {
    v as i32
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-int `as` saturates (NaN is 0); that is the wanted result"
)]
pub(crate) fn to_usize(v: f32) -> usize {
    v as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_saturate() {
        assert_eq!(to_u8(-1.0), 0);
        assert_eq!(to_u8(300.0), 255);
        assert_eq!(to_u8(f32::NAN), 0);
        assert_eq!(to_u8(254.9), 254);
        assert_eq!(to_i8(-200.0), -128);
        assert_eq!(to_i8(200.0), 127);
        assert_eq!(to_i8(-126.9), -126);
        assert_eq!(to_u32(-5.0), 0);
        assert_eq!(to_u32(f32::INFINITY), u32::MAX);
        assert_eq!(to_u32(f32::NAN), 0);
        assert_eq!(to_i32(f32::NEG_INFINITY), i32::MIN);
        assert_eq!(to_i32(-2.5), -2);
        assert_eq!(to_usize(-1.0), 0);
        assert_eq!(to_usize(4095.5), 4095);
    }
}
