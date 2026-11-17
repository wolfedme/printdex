# Printdex Model View & Settings (Milestone 1, part B) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish milestone 1: open a model to see its parts and files, switch the library between grid and list, narrow it with filter chips and sorting, manage libraries and appearance in Settings (all seven colour themes, light/dark/system, translucency), see offline libraries for what they are, and give the interface the quiet motion DESIGN.md asks for.

**Architecture:** Builds on part A (`docs/superpowers/plans/2026-11-17-printdex-library-grid.md`, PR #36). The app gains a `Screen` (`Library`, `Model(ModelId)`, `Settings`) and a persisted `Settings` file. The engine gains model detail, library management and reachability, and structured search filters. Pure logic stays in testable modules (`settings`, `filters`, `motion`, `list`), views stay thin.

**Tech Stack:** as part A, plus serde + toml (settings file), opener (show in file manager / open with default app). No new GPU or unsafe code.

**Specs:** v1 design §8 (file handling: open-in-file-manager and default app are allowed), §9 (screens), DESIGN.md (model view, list view, settings, motion, blur), mock screens in `docs/design/mockups.html` (`model`, `list`, `settings`).

**Scope.** Only what milestone 1's data supports. Recipe card, tags, collections, print log and material colours need milestone 3 data and are not drawn (no empty placeholders for them). 3D preview and real thumbnails are milestone 2; the model view uses the neutral placeholder. Windows Mica/Acrylic is not reachable through iced 0.14's window settings and is left to the milestone 4 polish pass (issue filed).

## Global Constraints

- Everything from part A still holds: no `unsafe`, Fluent for every string, tokens only from `theme`, solid twin for every glass token, never touch model files.
- "Show in folder" and "Open" only launch the OS file manager or default app (v1 §8.6); nothing is moved, renamed or deleted. **Remove library** removes Printdex's records only and says so in its confirmation; files stay.
- Settings persist to `settings.toml` in the OS config folder (`--config-dir` overrides, for tests and screenshots). A missing or unreadable file falls back to defaults and never blocks startup.
- Window translucency: transparent window + compositor blur only where iced/winit can blur (KDE Plasma on Wayland/X11, macOS). GNOME, other Linux desktops, Windows and the "Off" setting use the solid set. Panes switch token sets at runtime; the window's transparency is fixed at startup.
- Motion follows DESIGN.md "Motion": only colour, alpha and offsets animate; nothing blocks input; a "Reduce motion" setting turns off movement and keeps short colour fades. Animations run from `update()` on events, driven by `window::frames()` only while something is animating.
- Library reachability checks run off the UI thread with a 3 s timeout, so an unplugged NAS never freezes the app.

## Review Focus

1. **Offline library** (NAS unmounted, USB drive unplugged): its models stay browsable from the database; the sidebar marks it "Offline"; Rescan reports offline instead of marking models missing; Retry works once it is back. Tests: Task 1 `library_status_offline_when_root_missing`; Task 8 `offline_library_is_marked_and_kept`.
2. **Removing a library** asks first, removes only Printdex's records, and leaves the files. Tests: Task 1 `remove_library_keeps_files`; Task 7 `remove_requires_confirmation`.
3. **A broken settings file** (truncated, unknown theme name, wrong types) falls back to defaults field by field. Test: Task 2 `settings_tolerate_bad_values`.
4. **Opening a model that disappeared** (removed by a rescan while it was on screen): the model view shows "This model is no longer in your library" and Esc returns to the grid. Test: Task 4 `missing_model_shows_notice`.
5. **Filters and search together**: chips and text combine with AND, chips survive scope changes, and the result count matches what is shown. Tests: Task 1 `search_filters_by_format_and_parts`, Task 6 `chips_combine_with_text`.

---

## File Structure

```
crates/printdex-core/src/db/models.rs     ModelDetail, parts/assets for one model
crates/printdex-core/src/db/search.rs     SearchFilter (formats, multi-part), SortOrder
crates/printdex-core/src/db/libraries.rs  delete, rename
crates/printdex-core/src/engine.rs        model(), remove_library(), rename_library(), library_status()
crates/printdex-app/src/settings.rs       Settings, load/save
crates/printdex-app/src/theme/presets.rs  all seven colour themes, glass + solid (generated from .impeccable/design.json)
crates/printdex-app/src/filters.rs        filter chips → SearchFilter
crates/printdex-app/src/motion.rs         durations, easing, reduce-motion, per-pane animation state
crates/printdex-app/src/list.rs           list-mode row geometry
crates/printdex-app/src/view/model.rs     model view
crates/printdex-app/src/view/list.rs      list mode
crates/printdex-app/src/view/settings.rs  settings screen
crates/printdex-app/src/view/widgets.rs   segmented control, chip, section label shared by views
```

---

### Task 1: Engine — model detail, library management, reachability, filters

- [x] Failing tests (`db/models.rs`, `db/search.rs`, `db/libraries.rs`, `tests/engine.rs`):
  - `model_detail_lists_parts_and_assets` — parts with format, size, rel_path sorted by path; assets likewise; missing files left out; `None` for an unknown id.
  - `search_filters_by_format_and_parts` — `formats: [ThreeMf]` returns only models with a present 3MF part; `multi_part: Some(true)` only models with ≥ 2 present parts; count matches.
  - `sort_by_recently_added` — newest `created_at` first, then name.
  - `remove_library_keeps_files` — library, models, parts and assets rows are gone (cascade), files on disk untouched, `LibraryRemoved` event sent.
  - `rename_library_changes_display_name` — blank names rejected.
  - `library_status_offline_when_root_missing` — `Online` for an existing folder, `Offline` when the root is gone or unreadable, without blocking longer than the timeout.
- [x] Implement `Db::model_detail`, `SearchFilter { formats, multi_part }`, `SortOrder { Name, RecentlyAdded }`, `search_models_in(library, input, &filter, sort, limit)` and `count_models(…)`; `Db::delete_library`, `Db::rename_library`; `Engine::model`, `remove_library`, `rename_library`, `library_status`. Add `EngineEvent::LibraryRemoved`/`LibraryRenamed`. Regenerate `.sqlx/`.
- [x] Commit `feat(core): model detail, library management, filters`.

### Task 2: Settings file and screens

- [x] Failing tests in `settings.rs`: `defaults_when_missing`, `round_trips`, `settings_tolerate_bad_values` (unknown colour theme → default, wrong type → default, other fields kept), `save_is_atomic` (write temp + rename).
- [x] `Settings { color_theme, mode: System|Light|Dark, view: Grid|List, sort, translucency: Auto|Off, reduce_motion }`, serde + toml; loaded at boot, saved after each change (off the UI thread).
- [x] `Screen { Library, Model(ModelId), Settings }` in `app.rs`, with tests: `enter_opens_selected_model`, `escape_returns_to_library`, `ctrl_comma_opens_settings`. Keyboard: Enter / double-click opens, Esc goes back, Ctrl+, opens Settings. `--mode` from part A becomes a one-off override that is not saved.
- [x] Commit `feat(app): settings file and screen routing`.

### Task 3: Colour themes and translucency

- [x] Generate `theme/presets.rs` from `.impeccable/design.json` `extensions.colorThemes` (a small script in `crates/printdex-app/tools/`, output committed). Seven presets, each with glass and solid sets in light and dark.
- [x] Failing tests: `seven_presets`, `every_preset_has_solid_twins` (solid tokens opaque), `glass_alphas_match_design` (rail 0.82, ground 0.86/0.84, …), `presets_never_change_status_or_material`, `accent_text_contrast_holds_in_every_preset` (accent-ink on accent-soft ≥ 4.5:1, solid set).
- [x] `Palette::new(theme, mode, Surface::Glass|Solid)`; `blur_supported()` decides from the OS (macOS; Linux when `XDG_CURRENT_DESKTOP` contains KDE). Window `transparent` + `blur` set at startup only when supported and translucency is Auto; the ground gets the glass fill, everything else as before.
- [x] Commit `feat(app): all colour themes, glass and solid sets`.

### Task 4: Model view

- [x] Failing tests on `update()`: `opening_model_loads_detail`, `detail_for_other_model_is_ignored` (stale load), `missing_model_shows_notice`, `selecting_part_updates_caption`.
- [x] View per mock: header with breadcrumb (library › folders), model name (display type), actions "Show in folder" (ghost) and "Open" (secondary, opens the selected part with the default app); main column with the large preview well (placeholder) and a caption for the selected part (file name · size · format), a **Parts** row of tiles (name, size in mono; selected tile ringed), and an **Other files** list (assets: images, PDFs, readmes) with "Open"; side column (384px) **Details**: library, folder (mono path), parts count, total size, added date, kind (folder / single file). Status bar: `Esc back to library`, `←→ part`, `O open`.
- [x] Commit `feat(app): model view`.

### Task 5: List mode

- [x] Failing tests in `list.rs`: `visible_rows_for_list` (44px rows, header excluded), `scroll_target_for_list_row`.
- [x] Grid/list segmented control in the toolbar (persisted). List view per mock: one 14px pane, header row (group-label type), rows with small placeholder thumbnail, **Name** (600), **Folder** (mono, ink-3), **Parts**, **Added**; selected row Signal Wash; virtualized like the grid. Up/Down move by one row, Enter opens.
- [x] Commit `feat(app): list mode`.

### Task 6: Filter chips and sort

- [x] Failing tests in `filters.rs`: `chips_to_filter`, `removing_last_chip_clears_filter`, `chips_combine_with_text`, `chips_survive_scope_change`.
- [x] "+ Filter" opens a small menu: **Format** (STL, 3MF, OBJ) and **Parts** (single file, multi-part). Active filters show as chips (grey key, value, remove icon) beside the search field; chips scale in per DESIGN.md. Sort pick list: Name, Recently added (persisted). Header line above the grid: "{library} · 212 of 1,284 models match" when filtered.
- [x] Commit `feat(app): filter chips and sort`.

### Task 7: Settings screen

- [x] Failing tests: `remove_requires_confirmation`, `rename_library_updates_sidebar`, `changing_colour_theme_saves_settings`, `system_mode_follows_os`.
- [x] View per mock: 200px section list (**Libraries**, **Appearance**, **About**); Libraries table (name, location in mono, status badge Up to date / Scanning n / m / Offline since …, model count, Rescan or Retry, Remove with an inline confirm "Remove {name} from Printdex? Your files stay where they are."), "Add library" button; Appearance: colour theme chips with two-tone swatches (all seven), Theme segmented System / Light / Dark, Translucency Automatic / Off (with a note when the desktop cannot blur), Reduce motion switch; About: version, data folder, licence. Rows capped near 760px.
- [x] Commit `feat(app): settings screen`.

### Task 8: Offline libraries

- [x] Failing tests: `offline_library_is_marked_and_kept`, `rescan_of_offline_library_shows_offline_not_error`, `retry_brings_library_back`.
- [x] Check every library's status at boot and before each rescan; offline rows in the sidebar use `on-rail-2` with "Offline since 14:02" and the slate offline badge; their models stay in the grid; Rescan becomes Retry.
- [x] Commit `feat(app): offline library state`.

### Task 9: Motion

- [x] Failing tests in `motion.rs`: `durations_match_design` (90/160/260/720 ms), `ease_out_curve_endpoints`, `reduce_motion_disables_movement_keeps_fades`, `hover_lift_settles`, `only_animating_requests_frames`.
- [x] Selection ring and fill cross-fade (160 ms), hover lift of the placeholder model by 4px with the contact shadow shrinking (160 ms), press sink 1px (90 ms), chip scale-in from 0.92 (180 ms), part-tile selection fade. `window::frames()` subscription only while an animation runs. Reduce motion: no lift, sink or scale; fades stay.
- [x] Commit `feat(app): motion`.

### Task 10: Run, screenshot, measure, document

- [x] Screenshots under Xvfb: model view, list mode with chips, settings (light and dark, two colour themes), offline library.
- [x] Re-measure start to browsable grid on the 100k-part bench library (part A: ~240 ms).
- [x] README: keyboard shortcuts, settings file location. File the Windows Mica issue (milestone 4). Close the plan's task issues from the PR.
