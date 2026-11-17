//! Renders a thumbnail of a mesh file to a PNG:
//! `cargo run -p printdex-render --example thumb -- model.stl out.png [petg|pla|tpu]`

use printdex_mesh::{Format, Limits, load};
use printdex_render::{Material, RenderOptions, render};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (input, output) = (&args[1], &args[2]);
    let format = match input
        .rsplit('.')
        .next()
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("3mf") => Format::ThreeMf,
        Some("obj") => Format::Obj,
        _ => Format::Stl,
    };
    let material = match args.get(3).map(String::as_str) {
        Some("petg") => Material::Petg,
        Some("pla") => Material::Pla,
        Some("tpu") => Material::Tpu,
        _ => Material::Unknown,
    };
    let t = std::time::Instant::now();
    let mesh = load(input.as_ref(), format, Limits::default()).expect("load");
    let loaded = t.elapsed();
    // DECIMATE=n reduces the mesh to at most n triangles first.
    let mesh = match std::env::var("DECIMATE").ok().and_then(|v| v.parse().ok()) {
        Some(n) => {
            let t = std::time::Instant::now();
            let before = mesh.triangles();
            let mesh = printdex_mesh::decimate(mesh, n);
            println!(
                "decimated {before} -> {} in {:?}",
                mesh.triangles(),
                t.elapsed()
            );
            mesh
        }
        None => mesh,
    };
    let opts = RenderOptions {
        material,
        ..RenderOptions::default()
    };
    let t2 = std::time::Instant::now();
    let cpu = render(&mesh, &opts);
    let cpu_time = t2.elapsed();
    println!(
        "{} triangles, load {loaded:?}, CPU render {cpu_time:?}",
        mesh.triangles()
    );
    let img = match printdex_render::GpuRenderer::new() {
        Some(gpu) => {
            let _ = gpu.render(&mesh, &opts); // warm up
            let t3 = std::time::Instant::now();
            let img = gpu.render(&mesh, &opts).expect("gpu render");
            println!("GPU render on {}: {:?}", gpu.description, t3.elapsed());
            img
        }
        None => cpu,
    };
    image::RgbaImage::from_raw(img.width, img.height, img.rgba)
        .expect("size")
        .save(output)
        .expect("save");
}
