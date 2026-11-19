# Vendored crates

## iced_tiny_skia 0.14.1

iced's software renderer (used when there is no capable GPU), copied from
crates.io 0.14.1 and wired in with `[patch.crates-io]` in the root
`Cargo.toml`. MIT licensed (see `iced_tiny_skia/LICENSE`).

**Only change:** `src/window/compositor.rs` passes the grouped damage regions
through `coalesce`, which draws many regions (more than 8), or regions that
cover at least half of their bounding box, as one rectangle.

**Why:** iced only merges damage regions that waste under 20,000 px². While
the model grid scrolls, every card moves, which leaves about 100 small,
overlapping regions per frame. `Renderer::draw` repaints every layer once per
region and clears a full-window clip mask for each, so a scroll frame cost
~90 ms (about 11 fps) on a 1,204-model library. With the patch it costs
~3 ms.

**Removing it:** when an iced release fixes damage grouping, delete this
folder and the `[patch.crates-io]` entry, then re-run the scroll measurement
described in the PR that added it.

The crate is excluded from the workspace, so Printdex's lints and `cargo fmt`
don't apply to it. Its upstream code keeps its own `unsafe` blocks
(image allocation); the Printdex patch adds none.
