//! GPU thumbnail rendering on a headless wgpu device.
//!
//! The GPU rasterizes the triangles (the expensive part for big meshes)
//! into a normal target and a depth-value target at the supersampled
//! size. Both are read back and finished on the CPU with the same
//! shading, occlusion, edge pass and downsampling as [`crate::render`],
//! so a thumbnail looks the same whichever path drew it.
//!
//! [`GpuRenderer::new`] only accepts hardware adapters: a software adapter
//! (llvmpipe, WARP) is no faster than the CPU rasterizer. Set
//! `PRINTDEX_GPU=off` to always use the CPU, or `PRINTDEX_GPU=software` to
//! accept software adapters (for testing).

use std::sync::mpsc;

use glam::Vec3;
use printdex_mesh::{Mesh, pack};
use wgpu::util::DeviceExt;

use crate::raster::{
    Frame, Image, RenderOptions, Target, Tri, finish, pack_px, prepare, wants_smooth_normals,
};

/// Vertices per buffer: 3M × 20 bytes stays well under wgpu's default
/// 256 MB buffer limit.
const CHUNK_VERTICES: usize = 3 << 20;
/// Bytes per vertex: x, y, z (clip space), raw depth, then the normal as
/// snorm8x4.
const VERTEX_BYTES: usize = 20;
/// Value the depth-value target is cleared to: "nothing here".
const EMPTY_DEPTH: f32 = -1.0e30;

const SHADER: &str = r#"
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) depth: f32,
};

struct FOut {
    @location(0) normal: vec4<f32>,
    @location(1) depth: f32,
};

@vertex
fn vs(@location(0) pos: vec4<f32>, @location(1) normal: vec4<f32>) -> VOut {
    var o: VOut;
    o.clip = vec4<f32>(pos.xy, pos.z, 1.0);
    o.normal = normal.xyz;
    o.depth = pos.w;
    return o;
}

@fragment
fn fs(i: VOut) -> FOut {
    var o: FOut;
    let len = length(i.normal);
    var n = vec3<f32>(0.0);
    if len > 0.0 {
        n = i.normal / len;
    }
    o.normal = vec4<f32>(n * 0.5 + 0.5, 1.0);
    o.depth = i.depth;
    return o;
}
"#;

/// Why a GPU render did not produce an image. The caller falls back to
/// the CPU.
#[derive(Debug)]
pub struct GpuError(pub String);

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GPU render failed: {}", self.0)
    }
}

impl std::error::Error for GpuError {}

/// A headless device and pipeline for thumbnails. Safe to share between
/// threads; renders may run concurrently.
#[derive(Debug)]
pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::RenderPipeline,
    /// Adapter name and backend, for logs.
    pub description: String,
}

/// What `PRINTDEX_GPU` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GpuMode {
    Off,
    Hardware,
    AnyAdapter,
}

fn mode_from_env() -> GpuMode {
    match std::env::var("PRINTDEX_GPU").as_deref() {
        Ok("off" | "cpu" | "0") => GpuMode::Off,
        Ok("software" | "any") => GpuMode::AnyAdapter,
        _ => GpuMode::Hardware,
    }
}

impl GpuRenderer {
    /// Opens a GPU for thumbnails; `None` when there is no suitable
    /// adapter (or `PRINTDEX_GPU=off`). Blocking.
    pub fn new() -> Option<GpuRenderer> {
        let mode = mode_from_env();
        if mode == GpuMode::Off {
            return None;
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            ..wgpu::InstanceDescriptor::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .ok()?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu && mode != GpuMode::AnyAdapter {
            return None;
        }
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("printdex thumbnails"),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..wgpu::DeviceDescriptor::default()
        }))
        .ok()?;
        // Errors are caught per render with error scopes; anything that
        // slips through must not panic the app.
        device.on_uncaptured_error(std::sync::Arc::new(|_e| {}));
        let pipeline = pipeline(&device);
        Some(GpuRenderer {
            device,
            queue,
            pipeline,
            description: format!("{} ({:?})", info.name, info.backend),
        })
    }

    /// Renders `mesh` like [`crate::render`], on the GPU.
    pub fn render(&self, mesh: &Mesh, opts: &RenderOptions) -> Result<Image, GpuError> {
        let normals = wants_smooth_normals(mesh, opts).then(|| printdex_mesh::corner_normals(mesh));
        let mut bytes: Vec<u8> = Vec::with_capacity(mesh.positions.len() * VERTEX_BYTES);
        // Clip-space z needs the depth range, known only after the pass:
        // store raw depth now and convert below.
        let frame = prepare(mesh, normals.as_deref(), opts, |t: Tri| {
            for ((px, depth), normal) in t.px.iter().zip(t.depth).zip(t.normal) {
                for v in [px.x, px.y, 0.0, depth] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
                bytes.extend(pack(normal.to_array()).map(i8::cast_unsigned));
            }
        });
        drop(normals);
        let Some(frame) = frame else {
            return Ok(Image::transparent(opts.width, opts.height));
        };
        to_clip(&mut bytes, &frame);
        let target = self.rasterize(&bytes, &frame)?;
        Ok(finish(target, &frame, opts))
    }

    fn rasterize(&self, vertices: &[u8], frame: &Frame) -> Result<Target, GpuError> {
        let d = &self.device;
        d.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        d.push_error_scope(wgpu::ErrorFilter::Validation);
        let result = self.draw(vertices, frame);
        let validation = pollster::block_on(d.pop_error_scope());
        let oom = pollster::block_on(d.pop_error_scope());
        if let Some(e) = validation.or(oom) {
            return Err(GpuError(e.to_string()));
        }
        result
    }

    fn draw(&self, vertices: &[u8], frame: &Frame) -> Result<Target, GpuError> {
        let d = &self.device;
        let size = wgpu::Extent3d {
            width: frame.w,
            height: frame.h,
            depth_or_array_layers: 1,
        };
        let texture = |label, format, usage| {
            d.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let readable = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let color = texture("normal", wgpu::TextureFormat::Rgba8Unorm, readable);
        let depth_value = texture("depth value", wgpu::TextureFormat::R32Float, readable);
        let depth = texture(
            "depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let buffers: Vec<wgpu::Buffer> = vertices
            .chunks(CHUNK_VERTICES * VERTEX_BYTES)
            .map(|chunk| {
                d.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("thumbnail vertices"),
                    contents: chunk,
                    usage: wgpu::BufferUsages::VERTEX,
                })
            })
            .collect();

        let row = (frame.w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = |label| {
            d.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(row) * u64::from(frame.h),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        };
        let color_out = readback("color readback");
        let depth_out = readback("depth readback");

        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
            let value_view = depth_value.create_view(&wgpu::TextureViewDescriptor::default());
            let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
            let attachment = |view, clear| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("thumbnail"),
                color_attachments: &[
                    attachment(&color_view, wgpu::Color::TRANSPARENT),
                    attachment(
                        &value_view,
                        wgpu::Color {
                            r: f64::from(EMPTY_DEPTH),
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        },
                    ),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..wgpu::RenderPassDescriptor::default()
            });
            pass.set_pipeline(&self.pipeline);
            for buf in &buffers {
                // Buffers hold at most CHUNK_VERTICES vertices.
                let count = u32::try_from(buf.size() / VERTEX_BYTES as u64).unwrap_or(u32::MAX);
                pass.set_vertex_buffer(0, buf.slice(..));
                pass.draw(0..count, 0..1);
            }
        }
        for (tex, out) in [(&color, &color_out), (&depth_value, &depth_out)] {
            enc.copy_texture_to_buffer(
                tex.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: out,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(frame.h),
                    },
                },
                size,
            );
        }
        self.queue.submit([enc.finish()]);

        let (tx, rx) = mpsc::channel();
        for out in [&color_out, &depth_out] {
            let tx = tx.clone();
            out.slice(..)
                .map_async(wgpu::MapMode::Read, move |r| drop(tx.send(r)));
        }
        drop(tx);
        d.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError(e.to_string()))?;
        for r in rx.iter() {
            r.map_err(|e| GpuError(e.to_string()))?;
        }

        let mut target = Target::new(frame.w, frame.h);
        {
            let colors = color_out.slice(..).get_mapped_range();
            let depths = depth_out.slice(..).get_mapped_range();
            // (`max(1)`: chunk sizes must not be 0; an empty target reads nothing.)
            let (w, row) = ((frame.w as usize).max(1), (row as usize).max(1));
            let unit = |c: u8| f32::from(c) / 255.0 * 2.0 - 1.0;
            // Readback rows are `row` bytes, padded past the w pixels.
            let rows = depths.chunks(row).zip(colors.chunks(row));
            let targets = target.depth.chunks_mut(w).zip(target.normal.chunks_mut(w));
            for ((depth_row, color_row), (z_row, n_row)) in rows.zip(targets) {
                let read = depth_row
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(color_row.as_chunks::<4>().0);
                for ((z, c), (z_out, n_out)) in read.zip(z_row.iter_mut().zip(n_row)) {
                    let z = f32::from_le_bytes(*z);
                    if z > EMPTY_DEPTH / 2.0 {
                        *z_out = z;
                        *n_out = pack_px(Vec3::new(unit(c[0]), unit(c[1]), unit(c[2])));
                    }
                }
            }
        }
        color_out.unmap();
        depth_out.unmap();
        Ok(target)
    }
}

/// Turns pixel positions into clip space and raw depth into a 0..1 depth
/// test value (nearest is 0), in place.
fn to_clip(bytes: &mut [u8], frame: &Frame) {
    let (w, h) = (frame.w as f32, frame.h as f32);
    let range = (frame.dmax - frame.dmin).max(1e-6);
    for v in bytes.as_chunks_mut::<VERTEX_BYTES>().0 {
        // x, y, z (unused until now), raw depth, then the normal.
        let [x, y, z, d, ..] = v.as_chunks_mut::<4>().0 else {
            continue;
        };
        let f = |b: &[u8; 4]| f32::from_le_bytes(*b);
        let (px, py, depth) = (f(x), f(y), f(d));
        *x = (px / w * 2.0 - 1.0).to_le_bytes();
        *y = (1.0 - py / h * 2.0).to_le_bytes();
        *z = (((frame.dmax - depth) / range).clamp(0.0, 1.0) * 0.999).to_le_bytes();
    }
}

fn pipeline(d: &wgpu::Device) -> wgpu::RenderPipeline {
    let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("thumbnail"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let layout = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor::default());
    let target = |format| {
        Some(wgpu::ColorTargetState {
            format,
            blend: None,
            write_mask: wgpu::ColorWrites::ALL,
        })
    };
    d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("thumbnail"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: VERTEX_BYTES as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x4, 1 => Snorm8x4],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            targets: &[
                target(wgpu::TextureFormat::Rgba8Unorm),
                target(wgpu::TextureFormat::R32Float),
            ],
        }),
        primitive: wgpu::PrimitiveState {
            cull_mode: None,
            ..wgpu::PrimitiveState::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::tests::cuboid;

    /// A GPU for tests: any adapter, including software ones, so the path
    /// is exercised wherever Vulkan/Metal/DX12 exists. `None` skips.
    fn gpu() -> Option<GpuRenderer> {
        if mode_from_env() == GpuMode::Off {
            return None;
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..wgpu::DeviceDescriptor::default()
        }))
        .ok()?;
        let pipeline = pipeline(&device);
        Some(GpuRenderer {
            device,
            queue,
            pipeline,
            description: String::new(),
        })
    }

    #[test]
    fn gpu_matches_cpu() {
        let Some(g) = gpu() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let m = cuboid([0.0; 3], [10.0, 20.0, 5.0]);
        let o = RenderOptions::default();
        let a = g.render(&m, &o).unwrap();
        let b = crate::render(&m, &o);
        assert_eq!((a.width, a.height), (b.width, b.height));
        let differ = a
            .rgba
            .chunks(4)
            .zip(b.rgba.chunks(4))
            .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        let total = (a.width * a.height) as usize;
        assert!(differ * 100 < total, "{differ} of {total} pixels differ");
    }

    #[test]
    fn gpu_empty_mesh_is_transparent() {
        let Some(g) = gpu() else {
            return;
        };
        let img = g
            .render(&Mesh::default(), &RenderOptions::default())
            .unwrap();
        assert!(img.rgba.iter().all(|&b| b == 0));
    }

    #[test]
    fn clip_maps_corners_and_depth() {
        let frame = Frame::for_test(400, 300, 0.0, 10.0);
        let mut v = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 10.0] {
            v.extend(f.to_le_bytes());
        }
        v.extend([0u8; 4]);
        to_clip(&mut v, &frame);
        let r = |i: usize| f32::from_le_bytes([v[i], v[i + 1], v[i + 2], v[i + 3]]);
        assert_eq!((r(0), r(4)), (-1.0, 1.0));
        assert_eq!(r(8), 0.0, "nearest depth tests first");
        assert_eq!(r(12), 10.0, "raw depth kept");
    }
}
