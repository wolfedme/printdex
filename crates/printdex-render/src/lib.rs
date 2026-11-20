//! Printdex rendering: the camera, lighting and material ramps shared by
//! thumbnails and the interactive viewer, and the CPU thumbnail renderer.

pub mod camera;
pub mod gpu;
pub mod material;
pub mod raster;
pub mod viewer;

pub use camera::{Camera, Framing, View};
pub use gpu::{GpuError, GpuRenderer};
pub use material::{Material, Ramp};
pub use raster::{Image, RenderOptions, Sphere, render, render_with_normals, wants_smooth_normals};
pub use viewer::{GpuViewer, Placement, home_view};
