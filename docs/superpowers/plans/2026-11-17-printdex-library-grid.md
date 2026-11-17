# Printdex Library Grid (Milestone 1, part A) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The first runnable Printdex desktop app: an iced window that opens the engine, lets you add a library folder, scans it with visible progress, and shows every model as a pane in a virtualized grid that filters on every keystroke of the search field. It follows the "Frosted print" design (Graphite signal palette, light and dark), uses Fluent for every string, and is keyboard-first.

**Architecture:** A new `printdex-app` crate (binary `printdex-app`) on iced 0.14 with its tokio executor, so the app and the engine share one runtime. The app is Elm-style: `State` + `Message` + `update()` + `view()`. Everything that can be reasoned about without a window lives in plain modules with unit tests: the theme tokens (`theme`), grid geometry and virtualization (`grid`), search sequencing (`search`), keyboard selection (`nav`), translations (`i18n`) and the update logic (`app`). `view` modules only turn state into widgets. Engine calls are `Task::perform`s; engine events arrive through a `Subscription` built from `Engine::subscribe()`.

**Tech Stack:** iced 0.14 (features `tokio`, `svg`, `advanced`), fluent-bundle 0.16 + unic-langid, rfd 0.15 (native folder picker, xdg portal on Linux), printdex-core, tokio, tracing. Fonts: Figtree and JetBrains Mono (SIL OFL 1.1, bundled TTFs, Latin subset). Icons: Lucide SVGs (ISC), bundled.

**Specs:** v1 design §7 and §9, architecture design §5.3, §7, §10–§12, and `DESIGN.md` (source of truth for every colour, size, radius and font).

**Scope split.** Milestone 1 (issue #17) is too large for one reviewable PR. This plan (part A) delivers the app shell, the theme, translations, adding and scanning libraries, the grid and search. **Part B** (its own plan, written when A is merged) covers the model view with its file list, list mode, filter chips, the Settings screen (theme switch, remove library, rescan interval), the offline-library state, OS blur and the motion language beyond selection. Part B is tracked as its own issue under #17.

## Global Constraints

- No `unsafe` code; the workspace keeps `unsafe_code = "forbid"` (Max's standing rule).
- Every user-visible string goes through Fluent (`locales/en/main.ftl`); no literal UI strings in view code. A test fails if a key used in code is missing from the English bundle.
- Every colour, size, radius and font comes from the `theme` module; widgets never hold raw values.
- Colours use the sRGB hex from `DESIGN.md` (`Color::from_rgba8`).
- Part A ships the **solid** token set only (opaque window). Translucency needs per-OS compositor blur (DESIGN.md "Blur by platform") and is part B. The solid set is also what DESIGN.md requires on GNOME and software rendering, which is Max's daily setup and the cloud test setup.
- Colour theme is data, not code: `ColorTheme` is an enum whose palette is a table entry, so adding the other six presets in part B is adding data. Light/dark/system is a separate `Mode` setting; part A follows the OS (`iced::system::theme` + `theme_changes`).
- Previews are milestone 2. Each pane shows the opaque placeholder model in the ASA/unknown neutral ramp on a `tray` well, as DESIGN.md prescribes for unknown material.
- The UI thread never waits on disk: all engine calls are async tasks, and the grid only builds widgets for visible rows.
- Search results are capped at `SEARCH_LIMIT = 5_000` per query; the status bar says when a result is capped ("Showing 5,000 of 12,345"). Paging past the cap is milestone 4 (performance pass).
- Fonts and icons are bundled with their licence texts; all new crates must pass `cargo deny check licenses`.
- Linux is the primary platform; CI keeps building and testing on Linux, Windows and macOS.

## Review Focus

1. **Typing fast in the search field.** Results of an older query must never overwrite a newer one. Test: Task 5, `stale_results_are_dropped`.
2. **Scanning while browsing.** A running scan must not block search or scrolling, and finishing a scan refreshes the visible results without losing the query or the selection when the selected model still exists. Tests: Task 6, `scan_finished_reruns_current_query`, `selection_survives_refresh`.
3. **Large libraries.** With 5,000 results the view builds widgets only for the visible rows plus one row of overscan. Test: Task 4, `visible_range_*`; manual check with the 100k bench library in Task 9.
4. **Adding a bad folder** (a folder inside an existing library, a non-UTF-8 path, a cancelled picker). The app shows a translated error line and keeps working. Test: Task 6, `add_library_error_is_shown`.
5. **Keyboard.** `/` and Ctrl+F focus search; Esc clears it; arrows move the selection across rows and columns and clamp at the edges; the selected pane scrolls into view. Tests: Task 7.

---

## File Structure

```
crates/printdex-core/src/db/search.rs     + library filter, total count, per-library counts
crates/printdex-core/src/engine.rs        + search_in(), count(), library_counts()
crates/printdex-app/
  Cargo.toml
  assets/fonts/        Figtree-{400,500,600,700}.ttf, JetBrainsMono-{400,500}.ttf, OFL texts
  assets/icons/        Lucide SVGs used by the app + LICENSE
  assets/placeholder.svg   neutral-ramp placeholder model
  locales/en/main.ftl
  src/main.rs          logging, fonts, iced::application wiring
  src/app.rs           State, Message, update(), subscription()
  src/i18n.rs          Fluent bundle, t(), t_args()
  src/theme.rs         tokens, Palette, ColorTheme, Mode, widget styles
  src/grid.rs          column count, row height, visible range, scroll target
  src/search.rs        query generations
  src/nav.rs           keyboard selection movement
  src/format.rs        numbers with grouping, paths for captions
  src/view/mod.rs      shell layout
  src/view/sidebar.rs  libraries, counts, scan progress
  src/view/toolbar.rs  title, search field, rescan
  src/view/grid.rs     virtualized model panes
  src/view/statusbar.rs counts and key hints
  src/view/empty.rs    no library / no results
```

---

### Task 1: Engine queries for the grid

**Files:** `crates/printdex-core/src/db/search.rs`, `crates/printdex-core/src/engine.rs`, `crates/printdex-core/tests/engine.rs`, `.sqlx/`.

- [ ] Write failing tests in `db/search.rs`:
  - `search_filters_by_library` — two libraries with a "Dragon" each; `search_models_in(Some(lib_a), "dragon", 50)` returns only lib A's.
  - `count_matches_search` — `count_models(None, "drag")` equals the number of hits without a limit; blank query counts all present models; missing models are not counted.
  - `library_counts_per_library` — returns present-model counts keyed by library id; a library with no models is absent (the app treats absent as 0).
- [ ] Implement `Db::search_models_in(library: Option<LibraryId>, input, limit)`, `Db::count_models(library, input)` and `Db::library_model_counts()` with `query!`/`query_as!` (filter as `(?1 IS NULL OR m.library_id = ?1)`). `search_models` becomes `search_models_in(None, …)`.
- [ ] Expose on `Engine`: `search_in`, `count`, `library_counts`. Add an engine test `search_in_scopes_to_library` in `tests/engine.rs`.
- [ ] Regenerate offline metadata with `cargo sqlx prepare --workspace`; commit `.sqlx/`.
- [ ] `cargo test -p printdex-core`, clippy clean. Commit `feat(core): library-scoped search, counts`.

### Task 2: App crate, fonts, icons, Fluent

**Files:** workspace `Cargo.toml`, `crates/printdex-app/{Cargo.toml,assets/**,locales/en/main.ftl,src/main.rs,src/i18n.rs}`, `deny.toml` (only if a new licence appears), `.github/workflows/ci.yml` (Linux system packages if the build needs them).

- [ ] Add the crate to the workspace with `iced = { version = "0.14", features = ["tokio", "svg", "advanced"] }`, `fluent-bundle`, `unic-langid`, `rfd`, `printdex-core`, `tokio`, `tracing`, `tracing-subscriber`, `anyhow`.
- [ ] Bundle fonts (TTF, Latin subset) and their OFL texts; load them with `.font(include_bytes!(…))`, default font Figtree.
- [ ] Failing tests in `i18n.rs`:
  - `english_bundle_parses` — the bundle loads with no errors.
  - `t_returns_message` and `t_args_formats_numbers` (`status-models = { $count } models`).
  - `missing_key_falls_back_to_key` — an unknown key renders as the key itself (never panics).
  - `every_key_used_in_code_exists` — scans `src/**/*.rs` for `t("…")` / `t_args("…"` and checks each key exists in `main.ftl`.
- [ ] Implement `i18n` with a thread-local `FluentBundle` (no Unicode isolation marks, so captions render cleanly).
- [ ] `main.rs` opens a blank window titled from Fluent. `cargo run -p printdex-app` starts. Commit `feat(app): iced app crate with fonts and Fluent`.

### Task 3: Theme module (Graphite signal, light and dark)

**Files:** `crates/printdex-app/src/theme.rs`.

- [ ] Failing tests:
  - `hex_parses_rgb_and_rgba` — `hex("#9f8600")` and `hex("#f7edb2cc")` give the right `Color`.
  - `solid_set_is_opaque` — every pane token in the solid palettes (`ground`, `surface`, `tray`, `rail`, `rail_hi`, `edge`) has alpha 1.0, light and dark.
  - `mode_switches_palette` — `Palette::new(ColorTheme::GraphiteSignal, Mode::Dark).ink` is the dark ink, `Mode::Light` the light ink.
  - `status_colours_do_not_depend_on_colour_theme` — `ok`/`fail`/`partial`/`offline` come from a shared table (guards the "theme never changes status colours" rule).
- [ ] Implement tokens exactly from DESIGN.md: colours (solid set), spacing `S1..S9`, radii (`PANE 14`, `WELL 9`, `CONTROL 10`, `CHIP 999`), type sizes (display 24, title 19, section 15, body 13, caption 12, group 11, data 11), sidebar width 232, status bar 30, button 32.
- [ ] `Palette` → `iced::Theme::custom` plus style functions used by views: `pane`, `pane_selected` (Signal Wash fill + 2px accent ring), `well`, `rail`, `rail_item(active)`, `toolbar`, `statusbar`, `search_field`, `button_primary`, `button_secondary`, `button_ghost`, `scrollbar`.
- [ ] Commit `feat(app): theme tokens for Frosted print, Graphite signal`.

### Task 4: Grid geometry and virtualization

**Files:** `crates/printdex-app/src/grid.rs`.

Pane size follows DESIGN.md: five columns at the 1280px mock width with a 232px sidebar and 20px content padding; column gap 14, row gap 16. Panes have a minimum width of 188px and grow to fill the row.

- [ ] Failing tests:
  - `columns_for_width` — 1048px content → 5 columns; 400px → 2; tiny widths → 1.
  - `rows_for_items` — 0 items → 0 rows; 11 items in 5 columns → 3 rows.
  - `visible_range_at_top`, `visible_range_scrolled`, `visible_range_clamps_at_end` — given scroll offset, viewport height, row height and gaps, return the row range to build, with one row of overscan above and below.
  - `scroll_target_keeps_row_visible` — `None` when the row is fully visible, otherwise the offset that brings it to the nearest edge.
- [ ] Implement as pure functions over a `GridMetrics { columns, pane_width, row_height }`. Commit `feat(app): grid geometry and virtualization math`.

### Task 5: Search sequencing

**Files:** `crates/printdex-app/src/search.rs`.

- [ ] Failing tests:
  - `stale_results_are_dropped` — start query 1, start query 2, results of 1 arrive → ignored; results of 2 arrive → accepted.
  - `same_text_does_not_restart` — re-issuing the same text and scope keeps the generation.
- [ ] Implement `SearchState { text, scope, generation, results, total, loading }` with `begin(text, scope) -> Option<Ticket>` and `accept(ticket, hits, total) -> bool`. Every keystroke runs a query (engine search is ~5 ms p95 at 100k parts); no debounce timer. Commit `feat(app): search generations`.

### Task 6: App state, update and engine wiring

**Files:** `crates/printdex-app/src/app.rs`, `src/main.rs`.

- [ ] Failing tests on `update()` (no window needed; tasks are not run):
  - `boot_without_libraries_shows_empty_state`.
  - `search_input_starts_query` — `SearchChanged("dra")` sets the text and returns a task.
  - `scan_progress_updates_sidebar` — `Engine(ScanProgress{…})` sets the per-library progress; `ScanFinished` clears it.
  - `scan_finished_reruns_current_query` — keeps the query text and scope.
  - `selection_survives_refresh` — the selected model id is kept when it is still in the new results and cleared otherwise.
  - `add_library_error_is_shown` — `LibraryAdded(Err(CoreError::LibraryOverlaps{…}))` sets a translated error line; `PickerCancelled` changes nothing.
  - `selecting_library_scopes_search`.
- [ ] Implement `Message` (`Booted`, `Engine(EngineEvent)`, `SearchChanged`, `Results`, `Counts`, `SelectScope`, `PickLibrary`, `LibraryPicked`, `LibraryAdded`, `Rescan`, `Scroll`, `Resized`, `Key`, `Select`, `ThemeMode`), boot (open engine at `Engine::default_data_dir()`, or `--data-dir` for tests and screenshots), subscription (engine events via `Subscription::run_with`, keyboard, window resize, system theme changes). Adding a library starts its first scan right away.
- [ ] Commit `feat(app): state, update and engine wiring`.

### Task 7: Keyboard navigation

**Files:** `crates/printdex-app/src/nav.rs`, `app.rs`.

- [ ] Failing tests: `right_moves_one`, `down_moves_one_row`, `down_on_last_row_clamps`, `left_at_start_stays`, `first_arrow_selects_first_item`, `home_end`.
- [ ] Implement `nav::step(selected: Option<usize>, key, columns, len) -> Option<usize>`. Wire keys in `app`: `/` and Ctrl+F focus the search field, Esc clears the search (or blurs it when empty), arrows/Home/End move the selection and scroll it into view using `grid::scroll_target`. Arrow keys are ignored while the search field has focus except Down, which moves focus into the grid.
- [ ] Commit `feat(app): keyboard navigation`.

### Task 8: Views

**Files:** `crates/printdex-app/src/view/*.rs`, `src/format.rs`, `assets/placeholder.svg`, `assets/icons/*`.

- [ ] `format.rs` tests first: `groups_thousands` (`12345` → `12,345` in English), `parent_path` (`Minis/Dragons/Red` → `Minis/Dragons`; top-level → empty).
- [ ] Shell: sidebar (232px, `rail`), main column with toolbar (12×20 padding), content (16×20) and a 30px status bar.
- [ ] Sidebar: group label "LIBRARIES", "All models" row and one row per library with counts in tabular figures; active row Signal Wash; "Add library" button; scan progress at the foot ("Scanning Minis · 1,234 of 5,000").
- [ ] Toolbar: scope title (title type), search field with search icon and `/` hint, "Rescan" secondary button.
- [ ] Grid: a `scrollable` with `on_scroll`; only visible rows are built, with fixed-height spacers above and below. Model pane: `surface` fill, 1px `edge`, 14px corners, 8px padding; `tray` well with the placeholder; parts count chip when > 1; name (600, max two lines); parent path in mono `ink-3`. Selected: Signal Wash + 2px accent ring. Click selects.
- [ ] Empty states: no library ("Add a folder with your models" + primary "Add library"), no results ("No models match …").
- [ ] Status bar: model count (capped note when needed), library count, key hints.
- [ ] Commit `feat(app): library grid views`.

### Task 9: Run, screenshot, measure, document

- [ ] Generate a library with `printdex bench gen` (2,000 parts) and a small hand-made one; run the app under Xvfb with Mesa llvmpipe; take screenshots in light and dark; check typing in search and arrow keys.
- [ ] Measure app start to first painted grid on the 100k-part bench library (target < 1 s, v1 §7) and note the number in the PR.
- [ ] README: how to run the app. Close the plan's task issues from the PR. Commit `docs: run the desktop app`.
