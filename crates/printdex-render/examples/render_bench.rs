//! Times thumbnail renders: `cargo run --release -p printdex-render
//! --example render_bench -- [runs] model.stl...`. Prints the fastest
//! CPU and GPU (when available) render time per file, after one warm-up.

use std::time::{Duration, Instant};

use printdex_mesh::{Format, Limits, load};
use printdex_render::{GpuRenderer, RenderOptions, render};

/// The fastest run: the least disturbed by other work on the machine.
fn fastest(v: Vec<Duration>) -> Duration {
    v.into_iter().min().unwrap_or_default()
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let runs: usize = match args.first().and_then(|a| a.parse().ok()) {
        Some(n) => {
            args.remove(0);
            n
        }
        None => 9,
    };
    let gpu = GpuRenderer::new();
    let opts = RenderOptions::default();
    for path in &args {
        let mesh = load(path.as_ref(), Format::Stl, Limits::default()).expect("load");
        let time = |f: &dyn Fn()| {
            f();
            fastest(
                (0..runs)
                    .map(|_| {
                        let t = Instant::now();
                        f();
                        t.elapsed()
                    })
                    .collect(),
            )
        };
        let cpu = time(&|| drop(render(&mesh, &opts)));
        let gpu = gpu
            .as_ref()
            .map(|g| time(&|| drop(g.render(&mesh, &opts).expect("gpu"))));
        println!(
            "{:>40} {:>9} tris  cpu {:>8.2?}  gpu {}",
            path.rsplit('/').next().unwrap_or(path),
            mesh.triangles(),
            cpu,
            gpu.map_or("-".to_string(), |d| format!("{d:.2?}")),
        );
    }
}
