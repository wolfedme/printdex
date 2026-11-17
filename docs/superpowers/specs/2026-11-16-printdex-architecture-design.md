# Printdex — Architecture & Tech Stack

*Status: approved by Max · 2026-11-16*
*Builds on: [Printdex v1 design](2026-11-16-printdex-v1-design.md) (PR #1). That document owns product scope, data model, file-handling rules and performance targets; this one owns how the code is structured and which tools it uses. Where they overlap, this document refines §5 and §12 of the v1 design.*

## 1. Intent

Pin down the technical foundation before the first line of code, so milestone 1 starts on decisions that won't need undoing: the concurrency model, the crates, how engine and UI talk, and the project-level choices (translations, distribution, contributions, visual design) that get expensive to change once code exists.

The product goals stay as in the v1 design: native, lightweight, fast on large NAS libraries, fully local, Windows/macOS/Linux, GPL-3.0.

## 2. Goals, non-goals, anti-goals

**Goals**
- An engine (`printdex-core` and friends) with no UI dependencies, testable headless, driven by a CLI as easily as by the app.
- One concurrency model across the codebase that also fits the post-MVP network features (downloads, sync).
- Meet the v1 performance targets (start < 1 s, search < 50 ms per keystroke, scrolling never blocks, 300 MB STL never freezes the UI).
- Translatable UI from the first screen.
- Users can install from GitHub Releases and receive updates in-app.

**Non-goals (for now)**
- OS code signing / notarization (later, once there are outside users).
- Outside code contributions (see §9).
- Languages other than English at launch.

**Anti-goals**
- Anything that makes network calls the user didn't agree to (see the update check in §8).
- A server component, telemetry, or crash reporting that leaves the machine.

## 3. Constraints

- Rust stable; iced (chosen by Max) and the wgpu version iced pins.
- All dependencies GPL-3.0-compatible (MIT, Apache-2.0, BSD, Zlib, MPL-2.0, LGPL, GPL), enforced by `cargo-deny` in CI.
- Must run without a capable GPU (wgpu fallback adapter), per v1 design §3.
- Must stay responsive on high-latency SMB shares.

## 4. Approaches considered (concurrency model)

| Option | Trade-off | Verdict |
|---|---|---|
| **A. Plain threads** — rayon pool, dedicated DB thread, channels into iced; synchronous engine API | Simplest and lightest for disk/CPU work; easy to test. Networking later would add an async runtime in one module, so two models coexist. | Claude's original pick. Not chosen. |
| **B. Async (tokio) throughout** — async engine API, async DB, CPU work bridged to rayon | One model now and for networking later; matches iced's tokio executor. Every engine call is async; async SQLite is somewhat slower than rusqlite; async file I/O is a thread-pool wrapper anyway. | **Chosen by Max** ("bring in async now"). |

## 5. Architecture

### 5.1 Workspace

Cargo workspace, crates as in v1 design §5:

| Crate | Depends on | Role |
|---|---|---|
| `printdex-core` | tokio, sqlx, mesh | Libraries, scanner, fingerprints, watcher, DB + migrations, search, sidecars, file actions + undo journal, recipe/print-log logic, update check. Public async API + event stream. |
| `printdex-mesh` | (no async, no GPU) | STL/3MF/OBJ parsing, bounding box, decimation, 3MF embedded thumbnails. Pure, synchronous, CPU-bound. |
| `printdex-render` | wgpu, glam, mesh | Headless thumbnail renderer and the interactive viewport renderer, sharing camera/lighting/material code. |
| `printdex-app` | iced, core, render | UI only: views, state, theme, i18n. |
| `printdex-cli` | core, clap | `scan`, `search`, `stats`, `bench`. Used for debugging and the benchmark harness. |

Dependency direction is strictly downward; `core` never depends on `render` or `app`.

### 5.2 Runtime and work distribution *(decided by Max: async)*

- **tokio multi-thread runtime** owns all I/O: directory walks, file reads, DB access, watcher events, (later) HTTP. iced runs with its tokio executor so app and engine share one runtime.
- **CPU-heavy work** (fingerprint hashing, mesh parsing, decimation, image encoding) runs on a **rayon** pool. Async code hands work over and awaits a oneshot result, so long jobs never occupy tokio workers or the UI thread. *(Claude's call: rayon over `spawn_blocking`, because it bounds CPU parallelism to core count and work-steals well for many small hashes.)*
- **Thumbnail rendering** runs from the rayon pool, fed by a priority queue (on-screen items first, per v1 §7). It rasterizes on a headless wgpu device when the machine has a hardware GPU, and on a CPU rasterizer otherwise (no GPU, a software adapter such as llvmpipe/WARP, `PRINTDEX_GPU=off`, or any GPU error, after which the session stays on the CPU). Both paths share framing, shading, edge lines and downsampling, so thumbnails look the same. *(Decided by Max, 2026-11-17: GPU first, CPU fallback.)*
- Cancellation: every long job takes a cancellation token (`tokio_util::sync::CancellationToken`); scrolling away cancels queued thumbnails, closing a model cancels its preview load.

### 5.3 Engine ↔ UI contract

- `printdex-core` exposes an `Engine` handle (cheap to clone) with async methods: commands (`add_library`, `rescan`, `tag_model`, `log_attempt`, …) and queries (`search`, `model`, `parts`, …).
- State changes are published as an **event stream** (`tokio::sync::broadcast`): `ScanProgress`, `ModelAdded/Changed/Removed`, `ThumbnailReady`, `LibraryOnline/Offline`, `UpdateAvailable`. The app turns this into an iced `Subscription`; commands map to iced `Task`s.
- The UI holds no business state of its own beyond view state and a cache of what's on screen; the DB is the truth.
- The CLI uses the same `Engine`, which keeps the engine honest about being UI-free.

### 5.4 Database

- **sqlx** with SQLite (bundled `libsqlite3-sys`, FTS5 enabled), WAL mode.
- Migrations via `sqlx::migrate!`, embedded in the binary, run at startup.
- Compile-time checked queries using sqlx **offline mode** (`.sqlx/` metadata committed), so building needs no database. *(Claude's call)*
- Pool: one writer connection, several readers, so search never waits behind a scan's writes. Scan results are written in batched transactions.
- Search: FTS5 table over names, paths, tags, notes, recipe text; structured filters (material, status, …) as indexed columns.
- If profiling shows sqlx overhead hurts the < 50 ms search target, the fallback is moving search reads to `rusqlite` on a blocking thread behind the same API. *(Claude's call: not expected to be needed.)*

## 6. Tech stack

| Area | Choice | Notes |
|---|---|---|
| Async runtime | tokio, tokio-util | Shared with iced. |
| CPU pool | rayon | Bridged with oneshot channels. |
| Database | sqlx (SQLite, FTS5) | Offline query metadata committed. |
| Directory walk | jwalk | Parallel; collects path/size/mtime only. Wrapped so a slow share can't block the runtime. |
| Fingerprint hash | xxhash-rust (xxh3) | Head + tail 64 KiB + size, per v1 §7. Full hash on demand. |
| File watching | notify + notify-debouncer-full | Local disks only; NAS uses periodic rescans. |
| STL | hand-written parser over plain buffered reads (`BufReader`) | Binary + ASCII; hot path, format is trivial. Fuzzed. No memory mapping: `memmap2` needs `unsafe`, which the workspace forbids *(decided by Max, 2026-11-17)*. |
| 3MF | zip + quick-xml | Objects, plates (read-only info), embedded thumbnails. |
| OBJ | hand-written reader over `BufReader` | Only `v`/`f` lines are needed; streams under the same limits as STL. *(Milestone 2; tobj was the plan.)* |
| Decimation | meshopt (meshoptimizer bindings) | Threshold left to the builder. |
| Rendering | wgpu (iced's version) for thumbnails with a CPU rasterizer fallback; wgpu via iced's shader widget (viewport); glam | Shared camera, light and material ramps in `printdex-render`. |
| Images | image | Decode folder images and 3MF thumbnails; encode cache. |
| UI | iced (pinned release) | |
| i18n | fluent + fluent-bundle, with a small `t!`-style helper | See §7. |
| Sidecars / config | serde, toml | |
| IDs | uuid (v7) | |
| Errors | thiserror (libraries), anyhow (app, CLI) | |
| Logging | tracing, tracing-subscriber, tracing-appender | Rolling local log files only. |
| OS integration | trash, directories, opener | |
| CLI | clap | |
| Auto-update | see §8 | |
| HTTP (later) | reqwest (rustls) | Arrives with the update check, reused by downloads/sync. |
| Testing | built-in tests, insta (snapshots), proptest, cargo-fuzz for parsers, criterion for benches | |
| CI | GitHub Actions on Windows/macOS/Linux; clippy, rustfmt, tests, cargo-deny (licenses + advisories) | |

*(Claude's calls)* Pin released iced versions and upgrade deliberately; set an explicit MSRV in `Cargo.toml` and test it in CI; semver with tagged releases; millimetres as the only unit.

## 7. Translations *(decided by Max: Fluent from day one)*

- Every user-visible string in `printdex-app` goes through Fluent; no literal UI strings in view code.
- `.ftl` files under `printdex-app/locales/<lang>/`, embedded at build time. English only at launch.
- Locale follows the OS (via `sys-locale`), overridable in Settings; falls back to English per missing key.
- Engine errors and events carry codes and parameters, not English sentences, so the app can translate them. Log messages stay English.
- A CI check fails when a key used in code is missing from the English bundle. *(Claude's call)*

## 8. Distribution and updates *(decided by Max: unsigned releases with an auto-updater)*

- Releases built by GitHub Actions on tag and published to GitHub Releases: Windows installer, macOS `.dmg` (universal: Apple Silicon + Intel), Linux AppImage. Flatpak later. *(Claude's call)*
- No paid OS code signing for now. macOS builds are still **ad-hoc signed** (free, no Apple account), because Apple Silicon refuses fully unsigned downloads as "damaged" and updates need a stable signature. *(Claude's call)* Users will see Gatekeeper/SmartScreen warnings; the README explains how to open the app. Signing is revisited when there are outside users.
- **Auto-updater:** *(Claude's call)* **Velopack** (MIT; Rust SDK; Windows/macOS/Linux; delta updates, feed served from GitHub Releases). Fallback if it doesn't fit: `self_update` with our own per-platform install logic.
- **Update integrity without OS signing:** release artifacts are additionally signed with a Printdex-owned ed25519 key (minisign); the updater verifies before installing. The private key lives only in GitHub Actions secrets. *(Claude's call: without this, an unsigned auto-updater is an easy attack path.)*
- **Respecting "no network unless the user agreed":** the update check is the only background network call in v1. On first launch Printdex asks once whether to check for updates automatically; the answer is changeable in Settings, and "Check now" is always available. No data beyond the version request is sent. *(Claude's call, to keep the v1 offline rule)*
- Updates are downloaded in the background but only installed on restart, with the user's go-ahead.

## 9. Contributions *(decided by Max: no outside code for now)*

- Repository accepts issues, ideas and discussion; pull requests from outside are not accepted yet. `CONTRIBUTING.md` says so plainly, and a PR template points there.
- This keeps all copyright with Max, so relicensing stays possible. Revisit (CLA or DCO) before opening up.

## 10. Visual design *(decided by Max: custom design first)*

- A custom Printdex visual design is made **before** milestone 1's screens are built: colors, typography, spacing, iconography, and mockups of the library grid, model view, log-a-print form and settings, in light and dark.
- This is a separate piece of work (its own design session), and its output is a set of design tokens plus mockups.
- In code, the design lands as a `theme` module in `printdex-app` implementing iced's theming with those tokens; widgets use tokens, never raw colors or sizes.
- Engine work (scanner, DB, mesh, renderer, CLI) does not depend on the design and can start in parallel.
- *(Claude's call)* Plan engine-first: milestone 0 and the engine half of milestone 1 proceed immediately; UI tasks of milestone 1 wait for the approved design. The design session starts right away in its own thread, with Max deciding the look. If it stalls, milestone 1 UI is built on a placeholder token theme and restyled when the design lands.

## 11. Milestone order (adjusted)

Same milestones as v1 design §10, with the design work slotted in:

0. **Foundation:** workspace, CI (3 OSes, cargo-deny, MSRV), engine skeleton with event stream, DB + migrations, CLI `scan`/`search`, benchmark library generator. In parallel: visual design.
1. Libraries, scanner, fingerprinting, search, virtualized grid, model view — built on the approved design and Fluent from the start.
2. Thumbnails and 3D preview.
3. Tags, collections, notes, source, recipe, print log, sidecars.
4. NAS hardening, file actions + undo, performance pass, **release pipeline + auto-updater**.

## 12. Platforms and testing

- **Primary platform: Linux** (Max's daily OS). Each milestone is polished on Linux first (Wayland and X11); Windows and macOS must build and pass tests in CI at every milestone and get polish before a public release.
- **Where things get tested** *(Claude's call, from Max's answer)*:
  - Development and manual checks run in the Linux cloud environment: the app is built and run under Xvfb with Mesa's software renderer (llvmpipe), with screenshots for review. This doubles as the "no capable GPU" check.
  - Windows and macOS: GitHub Actions runners build, run all tests, including thumbnail rendering (GPU tests skip where no adapter exists; the CPU path always runs). Interactive macOS checks happen in an occasional Claude Code session on Max's Mac, mainly before releases.
  - NAS behaviour: no real NAS assumed. Tests run against a filesystem wrapper that injects latency, slow listings, disappearing mounts and unreliable change notifications; a local Samba share in CI is a later addition if the wrapper misses real-world issues.
  - Performance: the CLI `bench` command and a synthetic 100k-part library generator, run in CI to catch regressions; absolute targets are checked against the latency-injected setup.

## 13. Decided by Max (this session)

- Async (tokio) throughout, not plain threads.
- The stack in §6 as proposed.
- Fluent translations from day one.
- Unsigned GitHub releases with an auto-updater.
- No outside code contributions for now.
- Custom visual design before milestone 1 screens.
- Linux is the daily platform; testing in the cloud is preferred, with a Mac session when needed.

## 14. Left to the builder

- Exact versions; DB schema details beyond v1 §6; connection pool sizes.
- Thumbnail size/format and cache eviction; decimation threshold.
- How the walk is bridged into async (dedicated thread feeding a channel vs. `spawn_blocking`).
- Installer tooling per OS within the Velopack setup.
- Benchmark harness details and the synthetic library generator.
