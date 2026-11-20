//! Times 3D viewer frames: `cargo run --release -p printdex-render
//! --example viewer_bench -- [frames] model.stl...`. Draws each model
//! into a 1280×800 offscreen target while orbiting, waiting for the GPU
//! after every frame, and prints the fastest and median frame time.

use std::time::{Duration, Instant};

use printdex_mesh::{Format, Limits, load};
use printdex_render::{GpuViewer, Material, Placement, Sphere, View};

const W: u32 = 1280;
const H: u32 = 800;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let frames: usize = match args.first().and_then(|a| a.parse().ok()) {
        Some(n) => {
            args.remove(0);
            n
        }
        None => 60,
    };
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("adapter");
    println!("adapter: {}", adapter.get_info().name);
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
        ..wgpu::DeviceDescriptor::default()
    }))
    .expect("device");
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view_t = target.create_view(&wgpu::TextureViewDescriptor::default());
    for (n, path) in args.iter().enumerate() {
        let mesh = load(path.as_ref(), Format::Stl, Limits::default()).expect("load");
        let mesh = printdex_mesh::decimate(mesh, printdex_mesh::PREVIEW_TRIANGLES);
        let normals = printdex_mesh::corner_normals(&mesh);
        let sphere = Sphere::of(&mesh).expect("sphere");
        let mut viewer = GpuViewer::new(&device, format);
        viewer.set_mesh(&device, n as u64, &mesh, Some(&normals));
        let place = Placement {
            origin: [0.0, 0.0],
            width: W,
            height: H,
        };
        let mut view = View::default();
        let mut times: Vec<Duration> = Vec::new();
        for f in 0..=frames {
            view.orbit_px(7.0, 0.0);
            let t = Instant::now();
            viewer.prepare(&device, &queue, sphere, &view, Material::Pla, place);
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
            viewer.render(&mut enc, &view_t, [0, 0, W, H]);
            queue.submit([enc.finish()]);
            device
                .poll(wgpu::PollType::wait_indefinitely())
                .expect("poll");
            if f > 0 {
                times.push(t.elapsed());
            }
        }
        times.sort();
        println!(
            "{:>28} {:>8} tris  fastest {:>8.2?}  median {:>8.2?}",
            path.rsplit('/').next().unwrap_or(path),
            mesh.triangles(),
            times[0],
            times[times.len() / 2],
        );
    }
}
