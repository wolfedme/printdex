//! Printdex rendering: the camera, lighting and material ramps shared by
//! thumbnails and the interactive viewer, and the CPU thumbnail renderer.
#![cfg_attr(
    not(test),
    warn(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_possible_wrap
    )
)]

pub mod camera;
mod cast;
pub mod gpu;
pub mod material;
pub mod raster;
pub mod viewer;

pub use camera::{Camera, Framing, View};
pub use gpu::{GpuError, GpuRenderer};
pub use material::{Material, Ramp};
pub use raster::{Image, RenderOptions, Sphere, render, render_with_normals, wants_smooth_normals};
pub use viewer::{GpuViewer, Placement, home_view};
