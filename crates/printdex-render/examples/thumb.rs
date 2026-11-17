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
    let img = render(
        &mesh,
        &RenderOptions {
            material,
            ..RenderOptions::default()
        },
    );
    println!(
        "{} triangles, load {loaded:?}, render {:?}",
        mesh.triangles(),
        t.elapsed() - loaded
    );
    image::RgbaImage::from_raw(img.width, img.height, img.rgba)
        .expect("size")
        .save(output)
        .expect("save");
}
