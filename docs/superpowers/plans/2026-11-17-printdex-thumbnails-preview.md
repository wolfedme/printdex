# Printdex Thumbnails & 3D Preview (Milestone 2) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Every model and part shows a real preview in its material colour, previews appear progressively without ever blocking scrolling, and the model view gets an interactive 3D preview with orbit, pan, zoom and dimensions. A 300 MB STL never freezes the UI.

**Architecture:** Two new crates, as the architecture doc §5.1 laid out. `printdex-mesh` parses STL, 3MF and OBJ into a triangle soup with bounds and pulls embedded 3MF thumbnails; it is synchronous, pure and has no async or GPU code. `printdex-render` holds the shared camera, lighting and material ramp, a CPU rasterizer for thumbnails, and (part B) the wgpu pipeline for the interactive viewer. `printdex-core` gains a thumbnail service: a priority queue fed by what is on screen, a cache keyed by part fingerprint, and a `ThumbnailReady` event. The app asks for the visible models after every scroll, resize or search, and swaps the placeholder for the cached PNG when the event arrives.

**Tech Stack:** zip + quick-xml (3MF), a hand-written OBJ reader (only `v`/`f` lines; tobj was the plan), image (PNG encode/decode, folder images), glam (camera maths), wgpu through iced's shader widget (viewer, part B). STL is a hand-written parser over `BufReader`. No `unsafe`, no memory mapping.

**Specs:** v1 design §6 (part fields: dimensions, triangle count, thumbnail ref), §7 (thumbnails, 3D preview, performance targets), §9 (model view); architecture design §5.1–5.2, §6, §12; DESIGN.md "Secondary (material colours)", "Model pane", "Motion" (print-in). Epic: issue #18.

## Decisions made while planning (Claude's calls, flagged for review)

1. **Thumbnails are rasterized on the CPU, not with headless wgpu.** The architecture doc said "headless wgpu" for thumbnails. A small software rasterizer in `printdex-render` gives the same flat clay look (three-tone material ramp plus an edge line, DESIGN.md), needs no GPU device, cannot fight iced for the device, runs the same in CI on all three OSes without llvmpipe or WARP, and is testable pixel by pixel. A 1M-triangle part renders at 384×288 in tens of milliseconds on one core; the rayon pool runs several at once. The interactive viewer still uses wgpu (part B), and both share one camera, light and material module so a thumbnail and the first viewer frame match.
2. **Thumbnail format:** 384×288 PNG with a transparent background (model only; the app draws the well and the contact shadow, which keeps the hover lift working). Cache under the OS cache folder, `Printdex/thumbs/v1/`, file name from the source key. Safe to delete; the version folder is bumped when the look changes.
3. **Cache keys:** a rendered or embedded part thumbnail is keyed by the part fingerprint (size + xxh3) so a moved or renamed file reuses it; a folder-image cover is keyed by the image's path, size and mtime. Nothing about thumbnails goes into the database except measurements (below), so a deleted cache rebuilds by itself.
4. **Model cover order (v1 §7):** a 3MF part's embedded thumbnail, then an image in the model folder (names containing cover, preview, thumb or render win, then the first by path), then a render of the first part by path.
5. **Measurements:** parsing for a thumbnail also yields the part's bounding box and triangle count. A migration adds `dim_x/dim_y/dim_z` (mm), `triangles` and `measured_fp` (the fingerprint they belong to) to `parts`. The model view shows them.
6. **Memory budget for huge files:** STL is read in one buffered pass into a triangle soup (36 bytes per triangle). Jobs take permits from a byte-weighted semaphore (default 768 MB of mesh in flight), so four 300 MB STLs never load at once. A file whose triangle count exceeds a hard cap (50M) is refused and shows the placeholder with "Too large to preview".
7. **Failures are remembered:** a part that fails to parse gets a small `.fail` marker next to where its thumbnail would be, so it is not retried on every scroll; a rescan that changes the fingerprint clears it naturally.

## Global Constraints

- No `unsafe` (the workspace forbids it). No memory mapping. Files are opened read-only; Printdex never writes into a library.
- Nothing touches the disk or parses on the UI thread. Parsing and rasterizing run on rayon via `run_cpu`; cache reads and writes run on tokio's blocking pool.
- Visible items first: each new "visible set" from the app reorders the queue and drops queued jobs that left the screen (jobs already running finish and are cached).
- Previews are opaque and drawn in the material ramp. Until milestone 3 knows materials, every model uses the ASA / unknown ramp.
- Parsers treat every file as hostile: declared counts are checked against the file size, 3MF entries are read with a decompressed-size cap, indices are bounds-checked, NaN/inf coordinates are dropped. Fuzz-style tests feed truncated and random bytes and must never panic.
- Fluent for every new string. Tokens only from `theme`.

## Review Focus

1. **Scrolling never blocks:** `request_visible` returns immediately; thumbnails arrive as events. Test: Task 5 `visible_set_reorders_queue`, Task 6 `scroll_requests_visible_models`.
2. **Hostile and broken files:** truncated binary STL, ASCII STL with garbage, 3MF with a missing model part or a zip bomb, OBJ with out-of-range indices. Tests in Tasks 1–3 (`*_never_panics`, `rejects_*`).
3. **Huge files:** the byte-weighted budget and the triangle cap. Test: Task 5 `budget_limits_concurrent_bytes`.
4. **Cache correctness:** same fingerprint → same file; changed file → new thumbnail; deleted cache → re-rendered. Test: Task 5 `cache_keyed_by_fingerprint`.
5. **The look:** compare `docs/screenshots/m2-*.png` with the mockups.

---

## File Structure

```
crates/printdex-mesh/src/lib.rs          Mesh, Bounds, MeshError, load(path, format)
crates/printdex-mesh/src/stl.rs          binary + ASCII STL over BufReader
crates/printdex-mesh/src/threemf.rs      3MF: objects, components, build transforms, thumbnails
crates/printdex-mesh/src/obj.rs          OBJ via tobj
crates/printdex-render/src/lib.rs        re-exports
crates/printdex-render/src/camera.rs     framing camera shared by thumbnails and viewer
crates/printdex-render/src/material.rs   material ramps (DESIGN.md), shading
crates/printdex-render/src/raster.rs     CPU rasterizer → RGBA image
crates/printdex-core/migrations/0002_part_measurements.sql
crates/printdex-core/src/db/thumbs.rs    cover sources for models, measurements
crates/printdex-core/src/thumbs.rs       ThumbnailService: queue, budget, cache, events
crates/printdex-app/src/thumbs.rs        visible-set bookkeeping, handle cache
crates/printdex-app/src/view/*.rs        grid, list, model view use real previews
(part B) crates/printdex-render/src/gpu.rs, crates/printdex-app/src/viewer.rs
```

---

## Part A — Thumbnails

### Task 1: `printdex-mesh` crate and STL

- [x] New crate, workspace member, `[lints] workspace = true`. Types: `Mesh { positions: Vec<[f32; 3]> }` (three per triangle), `Mesh::bounds() -> Option<Bounds>`, `Mesh::triangles()`, `Bounds { min, max, size() }`, `MeshError { Io, Malformed(String), TooLarge { triangles } }`, `Limits { max_triangles }`.
- [x] Failing tests: `binary_stl_reads_triangles`, `ascii_stl_reads_triangles`, `binary_stl_starting_with_solid_is_binary` (header begins "solid" but size matches the binary formula), `rejects_truncated_binary`, `rejects_count_larger_than_file`, `drops_non_finite_vertices`, `bounds_of_cube`, `stl_never_panics` (1k random and truncated inputs).
- [x] Implement over `BufReader<File>` with an 80-byte header + count check, 50-byte records; ASCII by line tokenizer. Skip stored normals.
- [x] Commit `feat(mesh): STL parser`.

### Task 2: 3MF

- [x] Failing tests (fixtures built in the test with `zip::ZipWriter`): `reads_mesh_object`, `applies_build_item_transform`, `resolves_components`, `finds_thumbnail_via_rels`, `finds_bambu_plate_thumbnail` (`Metadata/plate_1.png` when no relationship), `rejects_missing_model_part`, `caps_decompressed_size`, `rejects_bad_indices`, `threemf_never_panics`.
- [x] Implement: find the model part through `_rels/.rels` (fallback `3D/3dmodel.model`), stream the XML with quick-xml, objects by id, components with transforms (depth-limited to 16 to stop cycles), build items with transforms. `thumbnail(path) -> Option<Vec<u8>>` returns PNG/JPEG bytes. Entry reads go through `Read::take(cap)`.
- [x] Commit `feat(mesh): 3MF parser and embedded thumbnails`.

### Task 3: OBJ

- [x] Failing tests: `reads_triangles`, `triangulates_quads`, `rejects_out_of_range_index` (tobj errors or our check), `obj_never_panics`.
- [x] Implement as a small streaming reader over `BufReader` (`v` and `f` lines, fan triangulation, negative indices), materials ignored (no `.mtl` reads). Chosen over tobj: less code than the glue, and the same limits as STL.
- [x] `load(path, format, limits)` dispatcher. Commit `feat(mesh): OBJ parser`.

### Task 4: `printdex-render` CPU rasterizer

- [x] Failing tests: `camera_frames_bounds` (all projected corners inside the image with margin), `empty_mesh_renders_transparent`, `cube_uses_three_ramp_tones` (top face clay-1, two visible sides clay-2/clay-3), `depth_test_keeps_nearest`, `edge_line_drawn_on_silhouette`, `render_is_deterministic`, `ramps_match_design_md` (hex values from the table).
- [x] `Camera::framing(bounds, aspect)`: three-quarter view from the front-right, 30° elevation, Z up (STL convention), orthographic so dimensions read true; the model's lowest point sits on a bed line 80% down where the app draws the contact shadow; `Material::{Petg, Pla, Tpu, Unknown}` with `ramp() -> [top, side, shade, edge]`; `shade(normal) -> rgb` blending shade → side → top by the light term; `render(&Mesh, Material, w, h) -> RgbaImage` with a z-buffer, 2×2 supersampling, then an edge pass on depth discontinuities in the edge colour.
- [x] Commit `feat(render): CPU thumbnail renderer`.

### Task 5: Thumbnail service in core

- [x] Migration `0002_part_measurements.sql`; `Db::cover_sources(model_ids)` (parts with format/fp, image assets) and `Db::set_measurements(part, fp, dims, tris)`; regenerate `.sqlx/`.
- [x] Failing tests (`thumbs.rs`, `tests/thumbs.rs`): `cover_prefers_embedded_then_image_then_render`, `cache_keyed_by_fingerprint`, `failed_part_is_not_retried`, `visible_set_reorders_queue`, `budget_limits_concurrent_bytes`, `ready_event_carries_path`, `measurements_stored`, `deleted_cache_rerenders`.
- [x] `Engine::thumbnails()` → `ThumbnailService` (one per engine): `request_visible(Vec<ThumbTarget>)` where `ThumbTarget = Model(ModelId) | Part(PartId)`, `cached(target) -> Option<PathBuf>` (cheap, for already-done covers), `EngineEvent::ThumbnailReady { target, path }` / `ThumbnailFailed { target, reason }`. Workers: up to `min(4, cores/2)` jobs, rayon for parse + raster + PNG encode, budget semaphore, atomic cache writes (temp + rename).
- [x] Commit `feat(core): thumbnail queue and cache`.

### Task 6: Previews in the app

- [x] Failing tests (`app.rs`, `thumbs.rs`): `scroll_requests_visible_models`, `ready_event_swaps_placeholder`, `missing_thumb_keeps_placeholder`, `model_view_requests_parts`.
- [x] Visible set = models in the built rows (grid) or rows (list), plus the open model's parts. Sent after scroll/resize/search/open, debounced to one per frame. `image::Handle::from_path` per ready path, kept in a bounded map (LRU 2k entries).
- [x] Grid pane, list thumbnail, model view preview well and part tiles show the thumbnail over the contact shadow; while pending, the placeholder plus a small "Rendering preview" label (DESIGN.md "Model pane"). Hover lift keeps working.
- [x] Model view caption adds dimensions (`48 × 32 × 20 mm`) and triangle count when known.
- [x] Screenshots `docs/screenshots/m2-grid-*.png`, `m2-model-*.png` from a generated sample library with real meshes.
- [x] Commit `feat(app): real previews in grid, list and model view`.

## Part B — 3D preview and polish (separate PR)

### Task 7: Decimation and preview mesh

- [ ] Pure-Rust vertex-clustering decimation in `printdex-mesh` down to a target (default 400k triangles) for interaction; full resolution on request. Tests: `decimate_hits_target`, `decimate_keeps_bounds`. (meshopt was the approved crate; it needs a C++ toolchain on every builder, so a small Rust clustering pass is tried first and meshopt stays the fallback if quality is poor.)

### Task 8: wgpu viewer

- [ ] iced `shader` widget with a vertex/index buffer upload, same camera/material/lighting as the CPU renderer, orbit (drag), pan (right/middle drag or Shift+drag), zoom (wheel), double-click resets, dimension readout. Parse and decimate on a worker; a loading state with the cached thumbnail shown meanwhile; closing the model cancels the load.
- [ ] Fallback when iced runs on its software renderer: show the CPU render of the current camera (re-rendered on release).
- [ ] Tests: camera maths (`orbit_clamps_elevation`, `zoom_limits`, `pan_moves_target`), `opening_large_file_never_blocks_update`.

### Task 9: Print-in motion, cache setting

- [ ] Print-in (DESIGN.md "Motion"): a newly rendered thumbnail builds up from the bed in 16 steps over 720 ms, stagger 30 ms capped at 330 ms; once per newly rendered thumbnail; Reduce motion turns it into a fade.
- [ ] Settings > Storage: cache size limit (default 1 GB), current usage, "Clear cache"; eviction by least-recently-used file time after each batch.
- [ ] Commit, screenshots, PR.
