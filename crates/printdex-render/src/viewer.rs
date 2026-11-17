//! The interactive 3D viewer on the GPU: draws a mesh into a region of a
//! wgpu render target (the app's window, through iced's shader widget).
//!
//! It looks like a thumbnail: the same orbit camera, material ramp,
//! world-fixed light, edge line and 2×2 supersampling. Pass one draws the
//! mesh into offscreen colour and depth-value textures at twice the
//! size; pass two finds edges, averages each 2×2 block and blends the
//! result over whatever is behind the viewer. The CPU fallback for the
//! same picture is [`crate::render`] with [`Framing::Orbit`].
//!
//! [`Framing::Orbit`]: crate::camera::Framing::Orbit

use glam::{Vec2, Vec3};
use printdex_mesh::{Bounds, Mesh};
use wgpu::util::DeviceExt;

use crate::camera::{Fit, View};
use crate::material::{Material, light_dir};
use crate::raster::{Sphere, edge_step};

/// Supersampling factor per axis (as for thumbnails), when the device
/// allows textures that large.
const SS: u32 = 2;
/// Positions per vertex buffer: whole triangles (a multiple of 3), and
/// 3M × 12 bytes stays well under wgpu's default 256 MB buffer limit.
const CHUNK_VERTICES: usize = 3 << 20;
const _: () = assert!(CHUNK_VERTICES % 3 == 0);
/// Value the depth-value texture is cleared to: "nothing here".
const EMPTY_DEPTH: f32 = -1.0e30;

const SCENE: &str = r#"
struct U {
    right: vec4<f32>,
    up: vec4<f32>,
    eye: vec4<f32>,
    light: vec4<f32>,
    // scale, offset x, offset y (supersampled pixels)
    fit: vec4<f32>,
    // width, height (supersampled), nearest and farthest depth
    size: vec4<f32>,
    top: vec4<f32>,
    side: vec4<f32>,
    shade: vec4<f32>,
    edge: vec4<f32>,
    // edge step, sRGB target, output origin x, y (target pixels)
    params: vec4<f32>,
    // supersampling, output width, height
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: U;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) depth: f32,
};

struct FOut {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
};

@vertex
fn vs(@location(0) p: vec3<f32>) -> VOut {
    let s = vec2<f32>(dot(p, u.right.xyz), dot(p, u.up.xyz));
    let d = dot(p, u.eye.xyz);
    let px = vec2<f32>(s.x * u.fit.x + u.fit.y, -s.y * u.fit.x + u.fit.z);
    let range = max(u.size.w - u.size.z, 1e-6);
    var o: VOut;
    o.clip = vec4<f32>(
        px.x / u.size.x * 2.0 - 1.0,
        1.0 - px.y / u.size.y * 2.0,
        clamp((u.size.w - d) / range, 0.0, 1.0) * 0.999,
        1.0,
    );
    o.world = p;
    o.depth = d;
    return o;
}

// material::shade: each tone holds flat, curved surfaces blend.
fn ramp(t: f32) -> vec3<f32> {
    if t <= 0.0 {
        return u.shade.rgb;
    } else if t < 0.4 {
        return mix(u.shade.rgb, u.side.rgb, t / 0.4);
    } else if t <= 0.6 {
        return u.side.rgb;
    } else if t < 0.78 {
        return mix(u.side.rgb, u.top.rgb, (t - 0.6) / 0.18);
    }
    return u.top.rgb;
}

@fragment
fn fs(i: VOut) -> FOut {
    // The face normal from the change of position across the triangle.
    var n = normalize(cross(dpdx(i.world), dpdy(i.world)));
    // Two-sided: many files have inconsistent winding.
    if dot(n, u.eye.xyz) < 0.0 {
        n = -n;
    }
    var o: FOut;
    o.color = vec4<f32>(ramp(dot(n, u.light.xyz)), 1.0);
    o.depth = i.depth;
    return o;
}
"#;

const COMPOSITE: &str = r#"
struct U {
    right: vec4<f32>,
    up: vec4<f32>,
    eye: vec4<f32>,
    light: vec4<f32>,
    fit: vec4<f32>,
    size: vec4<f32>,
    top: vec4<f32>,
    side: vec4<f32>,
    shade: vec4<f32>,
    edge: vec4<f32>,
    params: vec4<f32>,
    extra: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var color_t: texture_2d<f32>;
@group(0) @binding(2) var depth_t: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn depth_at(p: vec2<i32>) -> f32 {
    return textureLoad(depth_t, p, 0).r;
}

// raster::Target::edges: next to the background or a farther surface.
fn is_edge(p: vec2<i32>, here: f32, dims: vec2<i32>) -> bool {
    let dirs = array<vec2<i32>, 4>(vec2<i32>(1, 0), vec2<i32>(-1, 0), vec2<i32>(0, 1), vec2<i32>(0, -1));
    for (var r = 1; r <= 2; r++) {
        for (var k = 0; k < 4; k++) {
            let q = p + dirs[k] * r;
            if q.x < 0 || q.y < 0 || q.x >= dims.x || q.y >= dims.y {
                continue;
            }
            if here - depth_at(q) > u.params.x * f32(r) {
                return true;
            }
        }
    }
    return false;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let o = vec2<i32>(floor(pos.xy - u.params.zw));
    if o.x < 0 || o.y < 0 || f32(o.x) >= u.extra.y || f32(o.y) >= u.extra.z {
        discard;
    }
    let ss = i32(u.extra.x);
    let dims = vec2<i32>(textureDimensions(depth_t));
    var sum = vec3<f32>(0.0);
    var n = 0.0;
    for (var sy = 0; sy < ss; sy++) {
        for (var sx = 0; sx < ss; sx++) {
            let p = o * ss + vec2<i32>(sx, sy);
            let here = depth_at(p);
            if here <= -1.0e29 {
                continue;
            }
            var c = textureLoad(color_t, p, 0).rgb;
            if is_edge(p, here, dims) {
                c = u.edge.rgb;
            }
            sum += c;
            n += 1.0;
        }
    }
    if n == 0.0 {
        discard;
    }
    var rgb = sum / n;
    if u.params.y > 0.5 {
        rgb = to_linear(rgb);
    }
    return vec4<f32>(rgb, n / f32(ss * ss));
}
"#;

/// Size of [`Uniforms`] in bytes: twelve `vec4<f32>`.
const UNIFORM_BYTES: usize = 12 * 16;

/// Everything the shaders need for one frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Uniforms([f32; UNIFORM_BYTES / 4]);

/// Where and how to draw the viewer this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Top left of the viewer in the render target, in physical pixels
    /// (may be negative when scrolled partly out of view).
    pub origin: [f32; 2],
    /// Size of the viewer in physical pixels.
    pub width: u32,
    pub height: u32,
}

impl Uniforms {
    /// The uniforms for drawing a mesh with bounding `sphere` from `view`.
    pub fn new(
        sphere: Sphere,
        view: &View,
        material: Material,
        place: Placement,
        ss: u32,
        srgb_target: bool,
    ) -> Uniforms {
        let basis = view.camera.basis();
        let (w, h) = (place.width * ss, place.height * ss);
        let (c, d) = basis.project(sphere.center);
        let fit = Fit::orbit(c, sphere.radius, w, h, view.zoom, view.pan);
        let (dmin, dmax) = (d - sphere.radius, d + sphere.radius);
        let ramp = material.ramp();
        let rgb = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        let v3 = |v: Vec3| [v.x, v.y, v.z, 0.0];
        let c3 = |c: [u8; 3]| {
            let [r, g, b] = rgb(c);
            [r, g, b, 1.0]
        };
        let rows: [[f32; 4]; 12] = [
            v3(basis.right),
            v3(basis.up),
            v3(basis.toward_eye),
            v3(light_dir()),
            [fit.scale, fit.offset.x, fit.offset.y, 0.0],
            [w as f32, h as f32, dmin, dmax],
            c3(ramp.top),
            c3(ramp.side),
            c3(ramp.shade),
            c3(ramp.edge),
            [
                edge_step(fit.scale, dmin, dmax),
                if srgb_target { 1.0 } else { 0.0 },
                place.origin[0],
                place.origin[1],
            ],
            [ss as f32, place.width as f32, place.height as f32, 0.0],
        ];
        let mut out = [0.0; UNIFORM_BYTES / 4];
        for (i, row) in rows.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(row);
        }
        Uniforms(out)
    }

    fn bytes(&self) -> Vec<u8> {
        self.0.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

/// The screen position a mesh point lands on: for tests and hit checks.
pub fn project(sphere: Sphere, view: &View, width: u32, height: u32, p: Vec3) -> Vec2 {
    let basis = view.camera.basis();
    let (c, _) = basis.project(sphere.center);
    let fit = Fit::orbit(c, sphere.radius, width, height, view.zoom, view.pan);
    fit.to_pixel(basis.project(p).0)
}

/// The starting view for a model: the default orbit, zoomed so the model
/// as seen from there fills a `width` × `height` viewer (the bounding
/// sphere alone leaves flat or long models small).
pub fn home_view(sphere: Sphere, bounds: Bounds, width: f32, height: f32) -> View {
    let (min, max) = (Vec3::from_array(bounds.min), Vec3::from_array(bounds.max));
    let view = View::default();
    let basis = view.camera.basis();
    let (mut lo, mut hi) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { min.x } else { max.x },
            if i & 2 == 0 { min.y } else { max.y },
            if i & 4 == 0 { min.z } else { max.z },
        );
        let (s, _) = basis.project(corner);
        lo = lo.min(s);
        hi = hi.max(s);
    }
    let extent = (hi - lo).max(Vec2::splat(1e-6));
    let (c, _) = basis.project(sphere.center);
    let side = width.min(height);
    if side <= 0.0 {
        return view;
    }
    let usable = 1.0 - 2.0 * crate::camera::ORBIT_MARGIN;
    let sphere_scale = side * usable / (2.0 * sphere.radius);
    let fit_scale = (width * usable / extent.x).min(height * usable / extent.y);
    let zoom = (fit_scale / sphere_scale).clamp(1.0, crate::camera::MAX_ZOOM);
    // Centre the box, not the sphere (they differ in screen space only
    // by rounding for a box, but keep it exact).
    let mid = (lo + hi) / 2.0;
    let scale = sphere_scale * zoom;
    View {
        zoom,
        pan: Vec2::new(-(mid.x - c.x) * scale, (mid.y - c.y) * scale) / side,
        ..view
    }
}

struct Offscreen {
    width: u32,
    height: u32,
    color: wgpu::TextureView,
    depth_value: wgpu::TextureView,
    depth: wgpu::TextureView,
    composite: wgpu::BindGroup,
}

/// GPU state of the viewer: pipelines, the uploaded mesh and offscreen
/// textures. One per device; keeps one mesh at a time.
pub struct GpuViewer {
    scene: wgpu::RenderPipeline,
    composite: wgpu::RenderPipeline,
    composite_layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    scene_group: wgpu::BindGroup,
    srgb_target: bool,
    max_texture: u32,
    mesh: Option<(u64, Vec<(wgpu::Buffer, u32)>)>,
    offscreen: Option<Offscreen>,
    /// Supersampling of the current offscreen textures.
    ss: u32,
    /// Output rectangle in the target, for the scissor.
    place: Option<Placement>,
}

impl std::fmt::Debug for GpuViewer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuViewer")
            .field("mesh", &self.mesh.as_ref().map(|m| m.0))
            .field("ss", &self.ss)
            .finish_non_exhaustive()
    }
}

impl GpuViewer {
    /// Pipelines for drawing into targets of `format`.
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> GpuViewer {
        let uniform_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer scene"),
            entries: &[uniform_entry],
        });
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("viewer composite"),
            entries: &[uniform_entry, texture_entry(1), texture_entry(2)],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("viewer uniforms"),
            size: UNIFORM_BYTES as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewer scene"),
            layout: &scene_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });

        let module = |label, src: &str| {
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(src.into()),
            })
        };
        let scene_module = module("viewer scene", SCENE);
        let composite_module = module("viewer composite", COMPOSITE);
        let layout = |l: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[l],
                push_constant_ranges: &[],
            })
        };
        let opaque = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let scene = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewer scene"),
            layout: Some(&layout(&scene_layout)),
            vertex: wgpu::VertexState {
                module: &scene_module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 12,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &scene_module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[
                    opaque(wgpu::TextureFormat::Rgba8Unorm),
                    opaque(wgpu::TextureFormat::R32Float),
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
        });
        let composite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("viewer composite"),
            layout: Some(&layout(&composite_layout)),
            vertex: wgpu::VertexState {
                module: &composite_module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        GpuViewer {
            scene,
            composite,
            composite_layout,
            uniforms,
            scene_group,
            srgb_target: format.is_srgb(),
            max_texture: device.limits().max_texture_dimension_2d,
            mesh: None,
            offscreen: None,
            ss: SS,
            place: None,
        }
    }

    /// Uploads `mesh` unless the one with `id` is already there.
    pub fn set_mesh(&mut self, device: &wgpu::Device, id: u64, mesh: &Mesh) {
        if self.mesh.as_ref().is_some_and(|(have, _)| *have == id) {
            return;
        }
        // Drop the old buffers before allocating new ones.
        self.mesh = None;
        let buffers = mesh
            .positions
            .chunks(CHUNK_VERTICES)
            .map(|chunk| {
                let bytes: Vec<u8> = chunk
                    .iter()
                    .flatten()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                let buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("viewer mesh"),
                    contents: &bytes,
                    usage: wgpu::BufferUsages::VERTEX,
                });
                (buf, chunk.len() as u32)
            })
            .collect();
        self.mesh = Some((id, buffers));
    }

    /// Forgets the uploaded mesh (when the viewer closes).
    pub fn clear_mesh(&mut self) {
        self.mesh = None;
    }

    /// Supersampling used for an output of `width` × `height`: 2× unless
    /// the device's texture size limit forbids it.
    fn supersampling(&self, width: u32, height: u32) -> u32 {
        if width.max(height) * SS <= self.max_texture {
            SS
        } else {
            1
        }
    }

    /// Prepares a frame: (re)creates offscreen textures for the size and
    /// writes the uniforms for drawing `sphere` from `view`.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        sphere: Sphere,
        view: &View,
        material: Material,
        place: Placement,
    ) {
        let width = place.width.clamp(1, self.max_texture);
        let height = place.height.clamp(1, self.max_texture);
        let place = Placement {
            width,
            height,
            ..place
        };
        let ss = self.supersampling(width, height);
        let (w, h) = (width * ss, height * ss);
        if self
            .offscreen
            .as_ref()
            .is_none_or(|o| (o.width, o.height) != (w, h))
        {
            self.offscreen = Some(self.offscreen(device, w, h));
        }
        self.ss = ss;
        self.place = Some(place);
        let u = Uniforms::new(sphere, view, material, place, ss, self.srgb_target);
        queue.write_buffer(&self.uniforms, 0, &u.bytes());
    }

    fn offscreen(&self, device: &wgpu::Device, width: u32, height: u32) -> Offscreen {
        let texture = |label, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&wgpu::TextureViewDescriptor::default())
        };
        let read = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
        let color = texture("viewer colour", wgpu::TextureFormat::Rgba8Unorm, read);
        let depth_value = texture("viewer depth value", wgpu::TextureFormat::R32Float, read);
        let depth = texture(
            "viewer depth",
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("viewer composite"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&color),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&depth_value),
                },
            ],
        });
        Offscreen {
            width,
            height,
            color,
            depth_value,
            depth,
            composite,
        }
    }

    /// Draws the prepared frame into `target`, clipped to `clip` (x, y,
    /// width, height in target pixels; already inside the target).
    pub fn render(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip: [u32; 4],
    ) {
        let (Some(off), Some(place)) = (&self.offscreen, self.place) else {
            return;
        };
        // Scissor: the clip, within the viewer's own rectangle.
        let x0 = (clip[0] as f32).max(place.origin[0].max(0.0)).floor() as u32;
        let y0 = (clip[1] as f32).max(place.origin[1].max(0.0)).floor() as u32;
        let x1 = ((clip[0] + clip[2]) as f32)
            .min(place.origin[0] + place.width as f32)
            .max(0.0)
            .ceil() as u32;
        let y1 = ((clip[1] + clip[3]) as f32)
            .min(place.origin[1] + place.height as f32)
            .max(0.0)
            .ceil() as u32;
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        {
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
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewer scene"),
                color_attachments: &[
                    attachment(&off.color, wgpu::Color::TRANSPARENT),
                    attachment(
                        &off.depth_value,
                        wgpu::Color {
                            r: f64::from(EMPTY_DEPTH),
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        },
                    ),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &off.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..wgpu::RenderPassDescriptor::default()
            });
            pass.set_pipeline(&self.scene);
            pass.set_bind_group(0, &self.scene_group, &[]);
            if let Some((_, buffers)) = &self.mesh {
                for (buf, count) in buffers {
                    pass.set_vertex_buffer(0, buf.slice(..));
                    pass.draw(0..*count, 0..1);
                }
            }
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("viewer composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..wgpu::RenderPassDescriptor::default()
        });
        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        pass.set_pipeline(&self.composite);
        pass.set_bind_group(0, &off.composite, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::raster::tests::cuboid;
    use crate::{RenderOptions, render};

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        if matches!(
            std::env::var("PRINTDEX_GPU").as_deref(),
            Ok("off" | "cpu" | "0")
        ) {
            return None;
        }
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            ..wgpu::DeviceDescriptor::default()
        }))
        .ok()
    }

    /// Draws `mesh` with the viewer into a transparent RGBA texture and
    /// reads it back.
    fn draw(d: &wgpu::Device, q: &wgpu::Queue, mesh: &Mesh, view: &View, w: u32, h: u32) -> Vec<u8> {
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut v = GpuViewer::new(d, format);
        v.set_mesh(d, 1, mesh);
        let sphere = Sphere::of(mesh).unwrap();
        // The viewer sits at (10, 6) inside a larger target.
        let (tw, th) = (w + 20, h + 12);
        v.prepare(
            d,
            q,
            sphere,
            view,
            Material::Unknown,
            Placement {
                origin: [10.0, 6.0],
                width: w,
                height: h,
            },
        );
        let tex = d.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: tw,
                height: th,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view_t = tex.create_view(&wgpu::TextureViewDescriptor::default());
        let row = (tw * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let out = d.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row * th),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view_t,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..wgpu::RenderPassDescriptor::default()
            });
        }
        v.render(&mut enc, &view_t, [0, 0, tw, th]);
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &out,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(th),
                },
            },
            wgpu::Extent3d {
                width: tw,
                height: th,
                depth_or_array_layers: 1,
            },
        );
        q.submit([enc.finish()]);
        out.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        d.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = out.slice(..).get_mapped_range();
        let mut img = Vec::new();
        for y in 0..th {
            let o = (y * row) as usize;
            img.extend_from_slice(&data[o..o + (tw * 4) as usize]);
        }
        img
    }

    /// The viewer's own rectangle out of a [`draw`] result.
    fn crop(full: &[u8], w: u32, h: u32) -> Vec<u8> {
        let tw = (w + 20) as usize;
        let mut img = Vec::new();
        for y in 6..6 + h as usize {
            let o = (y * tw + 10) * 4;
            img.extend_from_slice(&full[o..o + w as usize * 4]);
        }
        img
    }

    #[test]
    fn viewer_matches_cpu_render() {
        let Some((d, q)) = device() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        let mesh = cuboid([0.0; 3], [10.0, 20.0, 5.0]);
        let mut view = View::default();
        view.orbit_px(60.0, -20.0);
        view.zoom_lines(2.0);
        view.pan_px(15.0, -10.0, 240.0);
        let (w, h) = (320, 240);
        let gpu = crop(&draw(&d, &q, &mesh, &view, w, h), w, h);
        let cpu = render(
            &mesh,
            &RenderOptions {
                width: w,
                height: h,
                material: Material::Unknown,
                camera: view.camera,
                framing: view.framing(),
            },
        );
        let differ = gpu
            .chunks(4)
            .zip(cpu.rgba.chunks(4))
            .filter(|(p, q)| p.iter().zip(*q).any(|(x, y)| x.abs_diff(*y) > 8))
            .count();
        let total = (w * h) as usize;
        assert!(cpu.rgba.chunks(4).filter(|p| p[3] > 0).count() > total / 20);
        assert!(differ * 100 < total, "{differ} of {total} pixels differ");
    }

    #[test]
    fn viewer_draws_nothing_outside_its_rectangle() {
        let Some((d, q)) = device() else {
            return;
        };
        // Zoomed far in: the model covers more than the viewer.
        let mesh = cuboid([0.0; 3], [10.0; 3]);
        let view = View {
            camera: Camera::default(),
            zoom: 8.0,
            pan: Vec2::ZERO,
        };
        let full = draw(&d, &q, &mesh, &view, 64, 48);
        let inside = crop(&full, 64, 48);
        assert!(inside.chunks(4).all(|p| p[3] == 255), "viewer fully covered");
        let covered = full.chunks(4).filter(|p| p[3] > 0).count();
        assert_eq!(covered, 64 * 48, "nothing drawn outside the viewer");
    }

    #[test]
    fn home_view_fills_a_wide_viewer() {
        // A flat, wide model in a wide viewer: zoomed in from the sphere.
        let mesh = cuboid([0.0; 3], [100.0, 100.0, 5.0]);
        let s = Sphere::of(&mesh).unwrap();
        let v = home_view(s, mesh.bounds().unwrap(), 700.0, 320.0);
        assert!(v.zoom > 1.2, "{}", v.zoom);
        // Every corner inside the viewer.
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { 0.0 } else { 100.0 },
                if i & 2 == 0 { 0.0 } else { 100.0 },
                if i & 4 == 0 { 0.0 } else { 5.0 },
            );
            let px = project(s, &v, 700, 320, p);
            assert!((0.0..=700.0).contains(&px.x) && (0.0..=320.0).contains(&px.y), "{px}");
        }
    }

    #[test]
    fn uniforms_put_model_centre_at_viewer_centre() {
        let mesh = cuboid([0.0; 3], [10.0; 3]);
        let s = Sphere::of(&mesh).unwrap();
        let p = project(s, &View::default(), 200, 100, s.center);
        assert!((p - Vec2::new(100.0, 50.0)).length() < 1e-3);
    }
}
