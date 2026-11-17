//! Printdex rendering: the camera, lighting and material ramps shared by
//! thumbnails and the interactive viewer, and the CPU thumbnail renderer.

pub mod camera;
pub mod material;
pub mod raster;

pub use camera::Camera;
pub use material::{Material, Ramp};
pub use raster::{Image, RenderOptions, render};
