# Printdex v1 — Design

*Status: approved by Max · 2026-11-16*

## 1. Intent

Printdex is a native desktop app that turns chaotic folders of 3D-print files into an organized, searchable library, and remembers how each model actually printed. It runs fully locally; it only uploads to backup storage the user configured (see the [backup design](2026-11-16-printdex-backup-design.md)).

**Why it exists.** Tools in this space (Manyfold, STL Shelf, and others) have roughly the right features, but each fails on delivery: they need a self-hosted server, are tied to one OS, are unmaintained, or are slow and overcomplicated. Printdex closes that gap: native on Windows, macOS and Linux, no server, lightweight, fast, and it speeds up the workflow instead of adding steps.

**Audience.** Hobbyists and makers with libraries ranging from a few hundred files on a local disk (Max) to many thousands of files / multiple TB on a NAS.

## 2. Goals, non-goals, anti-goals

**v1 goals**
- Add one or more library folders (local disk, external drive, NAS/SMB share) and get a browsable, searchable library.
- Thumbnails for every model and part; interactive 3D preview of a part.
- Tag models, group them into collections, add notes and a source URL.
- Per-model **recipe**: what a model needs to print well, shown at a glance.
- Per-model **print log**: each attempt with result, settings used, notes, optional photo.
- Feel instant on large NAS libraries (see §7 targets).
- Backup & restore of Printdex data and (opt-in) model files to a folder or remote of the user's choice ([backup design](2026-11-16-printdex-backup-design.md)).

**Post-MVP (planned; v1 data model must accommodate without migration pain)**
- Downloading/importing models from model sites (Printables, Thingiverse, MakerWorld…).
- Sync of Printdex data between machines.
- Reading print settings from slicer project 3MFs / G-code headers to prefill attempts (read-only).
- Optional "tidy library by tags" with a preview-then-apply flow.

**Never (anti-goals)**
- Slicing.
- Printer control or monitoring.
- Requiring an account, an upload or telemetry to use the app. Uploads happen only to storage the user configured.
- Modifying the contents of model files.
- Silent or automatic moving/renaming of user files.
- A server component of any kind for the core experience.

## 3. Constraints

- Language: Rust.
- Platforms: Windows, macOS, Linux desktop.
- Offline by default; network only for features the user explicitly sets up (backup targets, update check, later downloads/sync).
- Must work against slow, high-latency network shares where file-change notifications are unreliable.

- *(Claude's call)* Minimum platforms: Windows 10+, macOS 12+, Linux on both Wayland and X11. Rendering must work without a capable GPU: use wgpu's fallback/software adapter (WARP on Windows, llvmpipe on Linux) for thumbnails and preview, accepting slower rendering there.

## 4. Approaches considered (UI stack)

| Option | Trade-off | Verdict |
|---|---|---|
| **A. iced** (pure Rust, wgpu, Elm architecture) | One language; 3D preview and thumbnail rendering share the wgpu stack via a custom shader widget; actively maintained (System76 COSMIC). Fewer ready-made widgets (virtualized grid must be built); pre-1.0 API churn. | **Chosen by Max.** |
| B. Slint | Polished native-looking widgets, small footprint, fast to style. 3D viewport embedding less direct; GPL / royalty-free-with-credit / paid licensing. | Not chosen. |
| C. Tauri + web UI (Svelte + three.js) | Fastest to a pretty UI, huge ecosystem. Webview not native; WebKitGTK on Linux slow, WebGL unreliable; meshes must cross Rust↔JS; two languages. Works against "native and lightweight". | Rejected. |
| egui, GPUI | egui looks like a dev tool; GPUI has thin docs and is shaped by Zed's needs. | Dropped early. |

## 5. Architecture

A Cargo workspace with a strict split between engine and UI, so the UI is replaceable and a CLI is cheap.

- **`printdex-core`** (no UI deps): library management, scanner, fingerprinting, file watching, database + migrations, search, sidecar read/write, file actions (rename/move/trash + undo journal), recipe & print-log logic. Exposes an async-friendly API and an event stream (scan progress, model added/changed/removed, thumbnail ready).
- **`printdex-mesh`**: parsers for STL (binary + ASCII), 3MF, OBJ; streaming load; bounding box/dimensions; mesh decimation for preview; embedded-thumbnail extraction from 3MF.
- **`printdex-render`**: headless wgpu renderer for thumbnails and the interactive viewport renderer (shared camera/lighting/material code).
- **`printdex-app`**: iced application (views, state, theming). Thin: all logic lives in core.
- **`printdex-cli`** (optional in v1, cheap to add): `scan`, `search`, `stats` — useful for debugging and benchmarks.

Background work (scanning, hashing, thumbnailing) runs on worker pools off the UI thread; the UI subscribes to core events.

## 6. Data model

All entities have a stable UUID (v7) so future sync and import can merge records. Timestamps on every row (created/updated) for future sync conflict handling.

- **Library** — a root folder the user added. Fields: id, path, display name, kind hint (local / removable / network, auto-detected, user-overridable), sidecar mode (on/off), last full scan.
- **Model** — one folder inside a library, or a loose file that becomes its own model. Fields: id, library id, relative path, name (defaults to folder name), notes (Markdown), source (site + URL, optional), cover (chosen image or part thumbnail), derived print status (never printed / printed OK / failed / mixed), tags, collections.
  - *Model boundary rule:* a folder that directly contains printable files is a model; its subfolders' printable files are parts of the same model (common "STLs/", "supported/", "unsupported/" layouts). A loose printable file in a folder that also contains other model folders becomes a single-file model. The scanner's heuristics should be overridable per folder ("split into separate models" / "merge into parent").
- **Part** — each printable file (STL, 3MF, OBJ) in a model. *(Claude's call)* One part per file, even for multi-object/multi-plate 3MFs; the objects/plates inside a 3MF are stored as read-only sub-info (name, dimensions) for display, and attempts link to the file. Fields: id, model id, relative path, size, mtime, fingerprint, dimensions (x/y/z mm), triangle count, thumbnail ref.
- **Asset** — non-printable files in a model (images, PDFs, readmes, licenses). Images are candidates for the cover.
- **Tag** — flat, free-form, case-insensitive, autocomplete. Many-to-many with Model.
- **Collection** — hand-picked, named list; many-to-many with Model; order preserved. Nothing moves on disk.
- **Recipe** — 0..1 per model: the distilled "how to print this".
  - Requirements: structured settings (see below) marked as required/recommended.
  - Conditional notes: free text optionally scoped to a material, e.g. *"PETG: fails unless X and Y."*
  - Optional per-part overrides (e.g. one plate needs supports).
- **Print attempt** — 0..n per model, optionally linked to specific parts. Fields: date, result (success / failed / partial), printer (free text with autocomplete), settings used (structured + custom), what went wrong / notes, photos (stored in app data dir, referenced by id).
  - "Save as recipe" copies a successful attempt's settings into the recipe.
  - "Add as note" from a failed attempt creates a conditional recipe note.
- **Settings** (shared shape for recipes and attempts): material type (PLA, PETG, ABS, ASA, TPU, PA, …, custom), filament brand/color (free text), nozzle diameter, layer height, infill %, wall count, supports (none / normal / tree, + where), adhesion (none / skirt / brim / raft), nozzle temp, bed temp, orientation note, printer. Plus arbitrary custom key/value pairs. Structured fields are filterable.

**Storage**
- **SQLite** (via `rusqlite` or `sqlx`; builder's call) in the OS app-data dir is the source of truth; FTS5 for full-text search over names, paths, tags, notes, recipes.
- **Sidecar** `.printdex.toml` in each model folder, on by default per library: mirrors user-authored data (tags, collections membership by name, notes, source, recipe, attempts without photos). Written after DB changes (debounced). On scan, if a sidecar is newer than the DB record or the model is unknown, import it. Read-only folders silently fall back to DB-only. Schema-versioned.
  - *(Claude's call)* v1 assumes **one Printdex install per library**. The sidecar is a mirror and restore path, not a merge point. To stay sync-ready it still stores per-record `updated_at` and the writing install's id; if a scan finds a sidecar written by a different install, Printdex warns ("this library is also used by another Printdex") and imports per record by newest `updated_at` rather than whole-file mtime. Real multi-machine merging is part of the post-MVP sync feature.
- **Caches** (thumbnails, decimated preview meshes) in the OS cache dir, keyed by fingerprint; safe to delete.

## 7. Scanning, identity, performance

**Targets (reference: 100k parts / 2 TB on a 1 GbE SMB share; local SSD numbers should be far better)**
- App start to usable, browsable library: < 1 s (served from DB, no disk wait).
- Search/filter results update per keystroke: < 50 ms.
- Rescan of an unchanged library: dominated by directory listing; reads no file contents.
- Visible thumbnails appear progressively; scrolling never blocks on rendering.
- Opening a 300 MB STL in the 3D preview never freezes the UI.
- Idle memory and CPU should be modest (concrete numbers to be set by the builder after a first benchmark).

**Scanning**
1. Parallel directory walk collecting path, size, mtime only.
2. Diff against DB: unchanged (same path + size + mtime) → skip; otherwise fingerprint.
3. **Fingerprint** = size + fast hash (xxh3 or blake3) of the first and last 64 KiB. Full-content hash only on demand (e.g. duplicate confirmation).
4. Identity follows content: a part that disappears at one path and appears with the same fingerprint elsewhere is a move/rename; its model's tags, recipe and log follow. Same for whole model folders (matched by the set of part fingerprints).
5. Missing files are marked missing (not deleted) for a grace period, so an unmounted NAS doesn't wipe a library.

**Change detection**: `notify` file watching on local disks; network shares get a periodic incremental rescan (interval configurable) plus a manual Refresh. Removable/network libraries that are offline show as "offline" with data intact.

**Thumbnails**: priority queue fed by what's on screen; order of preference: 3MF embedded thumbnail → image in the model folder (for model cover) → headless wgpu render. Rendered once per fingerprint, cached locally.

**3D preview**: streaming parse on a worker, decimate above a triangle threshold for interaction, orbit/pan/zoom, show dimensions. Full-resolution optional.

## 8. File handling rules

1. Never modify model files.
2. Sidecar `.printdex.toml` is the only file Printdex writes into user folders, and can be turned off per library.
3. Rename model, move model to another folder/library, delete model: only on explicit user action, one at a time (multi-select allowed, but always confirmed). Delete goes to the OS trash. Moves/renames are recorded in an undo journal.
4. Everything generated (DB, thumbnails, previews, photos) lives in Printdex's own app dirs.
5. No automatic reorganization in v1.
6. Open-in-file-manager and "open with default app / chosen app" (e.g. a slicer) are allowed; that launches another program, it doesn't control it.

## 9. UI (v1 screens)

- **Library view** — sidebar: libraries, collections, tags, smart filters (never printed, failed, needs PETG…). Main: virtualized thumbnail grid (with list mode). Badges on cards: print status, key recipe facts (e.g. `PETG · 0.16 · supports`). Search bar with instant filtering and filter chips for structured fields.
- **Model view** — cover + parts grid, 3D preview of the selected part, recipe card at the top, tags/collections, notes, source link, print log timeline with "Log a print" button.
- **Log a print** — quick form; prefilled from the recipe; one-click result buttons; optional photo.
- **Settings** — libraries (add/remove, sidecar mode, rescan interval), theme (light/dark/system), cache size.

Keyboard-first: global search shortcut, arrow-key grid navigation, quick tag entry.

## 10. Decided by Max

- Name: Printdex. Native desktop, Windows/macOS/Linux, Rust.
- v1 = organizing + print success/failure tracking with settings and quirks.
- Downloads and sync after MVP; slicing and printer control never.
- UI framework: iced.
- Open source "for now"; no deadline.
  - License: **GPL-3.0** (set by Max in the repo). Dependencies must be GPL-3.0-compatible (MIT/Apache-2.0/BSD/Zlib/MPL-2.0/LGPL/GPL ok), enforced with `cargo-deny` in CI. *(Claude's note)* Since closed-source later isn't ruled out, Max should stay able to relicense: either keep copyright of outside contributions covered (e.g. a CLA/DCO-with-relicense clause) or accept that relicensing needs every contributor's consent; prefer permissive dependencies where a choice exists.
  - *(Claude's call)* Milestones, each shippable to Max for daily use:
    1. Libraries, scanner, fingerprinting, SQLite + FTS search, plain virtualized grid, model view with file list.
    2. Thumbnails (embedded → folder image → wgpu render) and 3D preview.
    3. Tags, collections, notes, source, recipe, print log, sidecars.
    4. NAS hardening (offline libraries, periodic rescan, move detection), file actions with trash + undo, performance pass against §7 targets.
    5. Backup & restore (added 2026-11-16; see the backup design). MVP ships after milestone 5.

## 11. Proposed by Claude, not objected to (confirm while reading)

- A model is a folder; loose files become single-file models.
- File handling rules in §8, including sidecar on by default.
- Recipe vs. attempts split, "save as recipe" flow, structured + custom settings.
- Flat tags + collections (no nested tag hierarchy in v1).
- Performance approach and targets in §7.
- Workspace split in §5.

## 12. Left to the builder

- Exact crates (rusqlite vs sqlx, walker crate, hashing crate, STL/3MF parsers vs hand-written), iced version pinning strategy.
- Thumbnail size/format, cache eviction policy, decimation algorithm and threshold.
- Exact folder-to-model heuristics beyond §6, and the override UI.
- Packaging/installers per OS, auto-update (if any), code signing.
- Benchmark harness and synthetic large-library generator.
