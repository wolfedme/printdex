# Printdex Tags, Collections, Recipe & Print Log (Milestone 3) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Printdex remembers what the user knows about each model: tags, collections, notes and a source link (part A), and how it prints: a recipe and a print log with results, settings and photos (part B). Cards show the material colour, the key recipe facts and the last print; the sidebar gets Collections, Smart filters and Tags. Everything the user writes is mirrored into a `.printdex.toml` sidecar next to the model, and read back on scan.

**Architecture:** One migration adds the user-authored tables (tags, collections, recipes, attempts, photos) and widens the FTS index to tags, notes and recipe text. `printdex-core` gets the types (`model_data.rs`), queries (`db/organize.rs`, `db/prints.rs`), a sidecar module (`sidecar.rs`: format, debounced writer, import on scan) and a photo store (`photos.rs`). Every user edit goes through an `Engine` method that writes the database, refreshes the model's FTS row, queues a sidecar write and emits `ModelDataChanged`. The app keeps only view state: the model view loads `ModelData` next to `ModelDetail`, the sidebar loads tag, collection and smart-filter counts, and search narrowing moves from "library scope" to a `Scope` (all, library, collection, tag, smart filter) plus filter chips.

**Tech Stack:** sqlx (SQLite, FTS5), serde + toml (sidecars), serde_json (settings column, already in the lock file), iced 0.14 widgets (`text_editor`, `combo_box`, `markdown`), rfd (photo picker). No new crates outside the existing tree except enabling iced's `markdown` feature (pulldown-cmark, MIT).

**Specs:** v1 design §6 (Tag, Collection, Recipe, Print attempt, Settings, sidecars), §8 (file rules: the sidecar is the only file Printdex writes into a library), §9 (library and model view, log a print, keyboard); architecture §5.3–5.4; DESIGN.md "Recipe card and print log", "Inputs / Fields", "Navigation (sidebar)", "Model pane", "Motion" (Log a print, chips), mockups `docs/design/mockups.html` (model and log screens). Epic: issue #19 (tasks: #67, #68, #69, #70, #75 in part A; #71–#74 in part B).

## Decisions made while planning (Claude's calls, flagged for review)

1. **Sidecar location.** A folder model's sidecar is `<model folder>/.printdex.toml`. A single-file model (a loose file next to other models) gets `.<file name>.printdex.toml` in the same folder (`.hook.stl.printdex.toml`), so loose files never share one file. Both names are hidden files on Linux/macOS.
2. **Sidecar content.** TOML, `schema = 1`, the writing install's id, the model id, and every user-authored record with its own `updated_at`: notes, source, tags, collection names, recipe, attempts (without photos). Import per record, newest `updated_at` wins (v1 §6). Import never deletes: a record missing from a sidecar stays in the database. A sidecar from another install shows a one-time warning per library ("This library is also used by another Printdex").
3. **When sidecars are written.** One second after the last change to a model (debounced per model), only when the library has sidecars on and is reachable. A failed write (read-only folder, offline share) is logged and the model stays database-only; the next edit tries again. The sidecar's mtime after writing is stored, so the next scan does not import Printdex's own write. Turning sidecars on for a library writes all its models that have user data, in the background.
4. **Import on scan.** The walk lets `.printdex.toml` and `.*.printdex.toml` through its hidden-file filter (as `FileKind::Sidecar`; never an asset). After the scan writes models, every sidecar whose mtime differs from the stored one is parsed on the blocking pool and imported. A model the database has never seen (a restored library) gets all its data back this way. Unparseable sidecars are logged and skipped, never overwritten until the user edits that model.
5. **Install id.** A new `meta` table holds `install_id` (uuid v7), created on first open.
6. **Tags** are stored once with their first-typed spelling (`name`) and matched by `name_key` (Unicode lower-case, trimmed, inner whitespace collapsed). A tag with no models left is deleted. Max 64 characters.
7. **Collections** are named lists, unique by `name_key`, order = order of adding (`position`). In a collection the library sorts by collection order by default (`SortOrder::Collection`, offered only there). Deleting a collection never touches models.
8. **Settings shape** (shared by recipe, per-part override and attempt): `PrintSettings` below; stored as JSON in one column, with `material` also in its own indexed column for filters. Field ranges (errors name them): nozzle 0.1–2.0 mm, layer 0.02–1.0 mm, infill 0–100 %, walls 0–50, nozzle temp 0–500 °C, bed temp 0–150 °C (the mockup's message), print time 0–10,000 min.
9. **Derived print status** from attempts: none → Never printed; latest Success → "Printed N×" (N = successes); latest Failed → "Failed"; latest Partial → "Partial"; the date is the latest attempt's. Smart filters: Never printed, Failed last time, Added this week (7 days).
10. **Card chips** (max two): the recipe material, then "supports" when the recipe needs supports, else the layer height (`0.20 mm`). No recipe: no chips.
11. **Material colour.** A model's preview material is its recipe material, else the material of its latest attempt; PETG, PLA and TPU have ramps, everything else (ABS, ASA, PA, PC, custom, unknown) uses the ASA/unknown ramp. The thumbnail cache key gains the material, so changing a recipe's material re-renders that model's previews (cache folder becomes `thumbs/v2/`).
12. **Photos** are copied into `<data dir>/photos/<attempt id>/<photo id>.<ext>` (PNG, JPEG, WebP; max 25 MB each, 8 per attempt), picked with the file dialog or dropped onto the form. They are not in sidecars (v1 §6). Deleting an attempt deletes its photos.
13. **Search** switches from compile-time-checked macros to one `QueryBuilder` shared by search and count, because the filter set (tags, collection, status, material, recency) no longer fits fixed SQL. The FTS table becomes a regular FTS5 table with columns `name, rel_path, tags, notes, recipe` (weights 10, 1, 4, 1, 1), kept in sync by triggers for name/path and by the engine for the rest.
14. **Notes** are Markdown, edited in a plain `text_editor`, shown with iced's markdown widget. Links open in the OS browser.
15. **Not in this milestone:** catalogue numbers (asked separately), reordering collections by drag, reading settings from slicer files, multi-install merging (post-MVP sync).

## Global Constraints

- No `unsafe` (the workspace forbids it).
- Never modify model files; the sidecar is the only file Printdex writes into a library, and only when the library's sidecar setting is on (v1 §8).
- The database is the source of truth; sidecars are a mirror and a restore path.
- Fluent for every new string; tokens only from `theme`; red only for failure; partial is always the triangle plus "Partial" (DESIGN.md named rules).
- Engine errors carry codes (`CoreError::TagInvalid`, `CollectionNameInvalid`, `CollectionExists`, `SettingOutOfRange { field, min, max }`, `PhotoInvalid`), never sentences.
- Search stays under 50 ms per keystroke on the 100k bench library (`printdex bench`), with tags and attempts generated for 20% of models.
- CI: PRs run Linux only; `.sqlx/` regenerated with sqlx-cli after query changes.

## Review Focus

1. **Sidecar round trip and restore:** delete the database, rescan, and every tag, note, collection, recipe and attempt comes back. Test: Task 4 `restore_from_sidecars_after_db_loss`.
2. **Our own write is not re-imported, a newer hand edit is:** Test: Task 4 `own_write_not_reimported`, `edited_sidecar_wins_per_record`.
3. **Read-only or offline library folders** never fail an edit. Test: Task 4 `readonly_folder_falls_back_to_db`.
4. **Hostile sidecars** (garbage, huge, wrong types, future schema) never panic and are never overwritten blindly. Test: Task 4 `bad_sidecar_is_skipped`.
5. **Tag case and Unicode:** "PETG", "petg " and "Petg" are one tag; "Ü" matches "ü". Test: Task 2 `tags_match_case_insensitively`.

---

## File Structure

```
crates/printdex-core/migrations/0003_user_data.sql   tags, collections, notes/source, recipes, attempts, photos, meta, wider FTS
crates/printdex-core/src/model_data.rs              PrintSettings, Material, Recipe, Attempt, ModelData, PrintStatus, LastPrint
crates/printdex-core/src/db/organize.rs             tags, collections, notes, source, FTS refresh
crates/printdex-core/src/db/prints.rs               recipes, attempts, photos rows, status queries
crates/printdex-core/src/db/search.rs               QueryBuilder search with Scope + filters; hits carry chips/status/material
crates/printdex-core/src/sidecar.rs                 format, writer task, import
crates/printdex-core/src/photos.rs                  photo store
crates/printdex-core/src/walk.rs, scan.rs           sidecars in the walk, import after apply
crates/printdex-core/src/thumbs.rs                  material in the cache key
crates/printdex-app/src/scope.rs                    sidebar scope (all, library, collection, tag, smart)
crates/printdex-app/src/view/sidebar.rs             Collections, Smart filters, Tags groups
crates/printdex-app/src/view/model.rs               side column: recipe, tags, kv, log; notes; header Log a print
crates/printdex-app/src/view/organize.rs            tag row, collection picker, source, notes editor
crates/printdex-app/src/view/recipe.rs              recipe card + recipe editor
crates/printdex-app/src/view/printlog.rs            timeline + Log a print form
crates/printdex-app/src/form.rs                     settings form state, parsing and validation
crates/printdex-app/src/view/grid.rs, list.rs       chips + last print on cards and rows
```

---

## Part A — Organize: tags, collections, notes, source, sidecars

### Task 1: Migration and core types

**Files:** `migrations/0003_user_data.sql`, `src/model_data.rs`, `src/ids.rs`, `src/db/mod.rs`, `.sqlx/`

**Interfaces — Produces:**
- ids: `TagId`, `CollectionId`, `AttemptId`, `PhotoId`.
- `enum Material { Pla, Petg, Abs, Asa, Tpu, Pa, Pc, Other(String) }` with `parse(&str)` (case-insensitive, "TPU 95A" → Tpu) and `label()`.
- `enum Supports { None, Normal, Tree }`, `enum Adhesion { None, Skirt, Brim, Raft }`, `enum AttemptResult { Success, Partial, Failed }`.
- `struct PrintSettings { material: Option<Material>, filament: Option<String>, nozzle_mm: Option<f32>, layer_mm: Option<f32>, infill_pct: Option<u8>, walls: Option<u8>, supports: Option<Supports>, supports_where: Option<String>, adhesion: Option<Adhesion>, nozzle_temp: Option<u16>, bed_temp: Option<u16>, orientation: Option<String>, printer: Option<String>, custom: BTreeMap<String, String> }` (serde, `skip_serializing_if` empty) with `validate() -> Result<(), CoreError>`.
- `struct ModelData { notes: String, source: Option<String>, tags: Vec<String>, collections: Vec<CollectionInfo>, recipe: Option<Recipe>, attempts: Vec<Attempt> }` (attempts newest first). `Recipe`/`Attempt` filled in part B.
- `Db::install_id() -> Uuid`.

- [ ] Tests: `migration_keeps_existing_models_searchable` (0002 db with rows → 0003 → name search still works), `settings_validate_ranges` (bed 850 → `SettingOutOfRange { field: "bed_temp", min: 0, max: 150 }`), `material_parse` ("petg", "TPU 95A", "Nylon" → Other), `install_id_is_stable`.
- [ ] Migration: `models` + `notes TEXT NOT NULL DEFAULT ''`, `source_url TEXT`, `data_updated_at INTEGER`, `sidecar_mtime_ns INTEGER`; tables `meta`, `tags`, `model_tags`, `collections`, `collection_models(position)`, `recipes(model_id PK, material, settings JSON, required JSON, from_attempt, updated_at)`, `recipe_notes`, `recipe_parts`, `attempts(model_id, printed_at, result, printer, material, settings JSON, duration_min, notes, parts JSON, updated_at)`, `attempt_photos`; indexes on `model_tags(tag_id)`, `collection_models(model_id)`, `attempts(model_id, printed_at)`, `recipes(material)`. Replace `models_fts` with the regular FTS5 table of decision 13 and new triggers; repopulate.
- [ ] Commit `feat(core): user data schema and types`.

### Task 2: Tags, collections, notes and source in the engine

**Interfaces — Produces (all on `Engine`, each emits `EngineEvent::ModelDataChanged(ModelId)` and, for list changes, `EngineEvent::OrganizeChanged`):**
`model_data(ModelId) -> Option<ModelData>`, `set_notes(ModelId, &str)`, `set_source(ModelId, Option<&str>)` (http/https only, else `SourceInvalid`), `add_tag(ModelId, &str)`, `remove_tag(ModelId, &str)`, `tags() -> Vec<TagCount { name, count }>` (by count desc, then name), `collections() -> Vec<CollectionInfo { id, name, count }>` (by name), `create_collection(&str) -> CollectionInfo`, `rename_collection(CollectionId, &str)`, `delete_collection(CollectionId)`, `add_to_collection(CollectionId, ModelId)`, `remove_from_collection(CollectionId, ModelId)`.

- [ ] Tests (`db/organize.rs`, `tests/engine.rs`): `tags_match_case_insensitively`, `unused_tag_is_deleted`, `tag_too_long_rejected`, `collection_names_unique`, `collection_keeps_adding_order`, `delete_collection_keeps_models`, `notes_and_tags_are_searchable` (search "gridfinity" finds a model tagged gridfinity; "warps" finds a note), `edits_emit_model_data_changed`.
- [ ] Every write bumps `models.data_updated_at` and refreshes the model's FTS `tags`/`notes`/`recipe` columns in the same transaction.
- [ ] Commit `feat(core): tags, collections, notes and source`.

### Task 3: Search scopes and filters

**Interfaces — Produces:** `enum Scope { All, Library(LibraryId), Collection(CollectionId), Tag(String), Smart(Smart) }`, `enum Smart { NeverPrinted, FailedLast, AddedThisWeek }`, `SearchFilter` gains `materials: Vec<Material>` (part B uses it), `SortOrder::Collection`; `Engine::search_in(&Scope, query, &SearchFilter, SortOrder, limit)` and `count(&Scope, query, &SearchFilter)` replace the `Option<LibraryId>` versions; `Engine::smart_counts() -> SmartCounts { never_printed, failed_last, added_this_week }`. `ModelHit` gains `material: Option<Material>`, `chips: Vec<Chip>` (part B fills), `last_print: Option<LastPrint>`.

- [ ] Tests: existing search tests ported to `Scope`; `scope_collection_in_collection_order`, `scope_tag`, `smart_never_printed`, `smart_added_this_week`, `count_matches_search_for_every_scope`.
- [ ] One `fn hits_query(builder, scope, fts, filter, sort, limit)` used by both search and count.
- [ ] Bench: `printdex bench` generates tags/notes for 20% of models; p95 stays < 50 ms (record numbers in the PR).
- [ ] Commit `feat(core): search by collection, tag and smart filter`.

### Task 4: Sidecars

**Interfaces — Produces:** `sidecar::Sidecar` (serde), `sidecar::path_for(&Library, &ModelRow) -> PathBuf`, `SidecarWriter` started by `Engine::open` (`queue(ModelId)`), `Engine::set_sidecars(LibraryId, bool)`, `EngineEvent::SidecarForeign { library }`. Scan: `FileKind::Sidecar`, `ScanSummary.sidecars_imported`.

- [ ] Tests (`sidecar.rs`, `tests/sidecars.rs`): `sidecar_round_trips` (serialize → parse equal), `folder_and_file_model_paths`, `writes_after_debounce`, `disabled_library_writes_nothing`, `readonly_folder_falls_back_to_db`, `own_write_not_reimported`, `edited_sidecar_wins_per_record`, `older_sidecar_does_not_override`, `restore_from_sidecars_after_db_loss`, `bad_sidecar_is_skipped` (garbage, 2 MB cap, `schema = 99`), `foreign_install_warns_once`, `sidecar_is_not_an_asset`.
- [ ] Writes are atomic (temp file in the same folder + rename), capped at 1 MB.
- [ ] Commit `feat(core): .printdex.toml sidecars`.

### Task 5: Organize in the app

**Files:** `scope.rs`, `view/sidebar.rs`, `view/model.rs`, `view/organize.rs`, `view/toolbar.rs`, `view/settings.rs`, `locales/en/*.ftl`

- [ ] Tests (`app.rs`): `selecting_collection_scopes_search`, `selecting_tag_scopes_search`, `model_data_changed_reloads_open_model_and_sidebar`, `t_focuses_tag_input`, `n_starts_editing_notes`, `escape_cancels_note_edit`, `adding_existing_tag_case_variant_shows_once`.
- [ ] Sidebar: Libraries, Collections (+ to create inline), Smart filters, Tags (top 12 by count, "Show all tags" expands), each with counts; the scope's title row above the grid gets Rename/Delete for a collection (delete confirms inline).
- [ ] Model view side column: tag row (chips with remove, "+ Tag" with `T`, autocomplete from existing tags via `combo_box`), key/value list (Collections with add/remove picker incl. "New collection…", Source link + edit, Folder). Main column: Notes section with Markdown display and `Edit` (`N`), `Ctrl+Enter` saves, `Esc` cancels.
- [ ] Settings › Libraries: "Write .printdex.toml files" switch per library, with a one-line explanation.
- [ ] Toast when another install's sidecar is found.
- [ ] Screenshots `docs/screenshots/m3-sidebar-*.png`, `m3-model-organize-*.png` (light and dark).
- [ ] Commit `feat(app): tags, collections, notes and source`.

## Part B — Recipe, print log, badges (separate PR)

### Task 6: Recipe and attempts in the engine

**Interfaces — Produces:** `struct Recipe { settings: PrintSettings, required: BTreeSet<String>, notes: Vec<RecipeNote { id, material: Option<Material>, text }>, parts: Vec<PartOverride { part: String /* file name */, settings: PrintSettings }>, from_attempt: Option<AttemptId>, updated_at: i64 }`; `struct Attempt { id, printed_at: i64, result: AttemptResult, settings: PrintSettings, duration_min: Option<u32>, notes: String, parts: Vec<String>, photos: Vec<PhotoRef { id, path }>, number: u32, updated_at }`; `Engine::set_recipe(ModelId, Option<Recipe>)`, `log_attempt(ModelId, NewAttempt) -> Attempt`, `update_attempt(Attempt)`, `delete_attempt(AttemptId)`, `recipe_from_attempt(AttemptId)`, `note_from_attempt(AttemptId, &str)`, `add_photo(AttemptId, &Path) -> PhotoRef`, `remove_photo(PhotoId)`, `printers() -> Vec<String>`.

- [ ] Tests: `log_attempt_numbers_in_order`, `latest_attempt_drives_status`, `recipe_from_attempt_copies_settings`, `note_from_failed_attempt_is_scoped_to_material`, `invalid_settings_rejected`, `photo_copied_and_deleted_with_attempt`, `photo_too_large_rejected`, `printers_autocomplete_distinct`, `recipe_and_attempts_in_sidecar`, `hits_carry_chips_and_last_print`, `material_filter` (`materials: [Petg]`).
- [ ] Commit `feat(core): recipes, print log and photos`.

### Task 7: Material-coloured previews

- [ ] Tests (`thumbs.rs`): `cache_key_includes_material`, `recipe_material_change_rerenders`.
- [ ] `ThumbnailService` looks up a model's material (recipe, else latest attempt) and renders parts and covers in that ramp; the viewer uses it too.
- [ ] Commit `feat: previews in the model's material colour`.

### Task 8: Recipe card, print log and Log a print

- [ ] Tests (`form.rs`, `app.rs`): `form_prefills_from_recipe`, `form_rejects_out_of_range_with_message`, `ctrl_enter_saves_attempt`, `l_opens_log_form`, `save_closes_form_and_reloads`, `cancel_discards`, `save_as_recipe_from_success`, `add_as_note_from_failure`, `dropped_image_attaches_photo`.
- [ ] Recipe card (DESIGN.md "Recipe card and print log"): 3-column label/mono value grid, notes with lightbulb (material-scoped) or info icon, "from attempt N · date"; Edit opens the recipe editor in the side column (same form as Log a print, plus required flags, notes and per-part overrides).
- [ ] Print log: entries with status icon + word, date, printer, "Attempt N", setting chips, notes, photo thumbnails (click opens the photo), "Save as recipe" on success, "Add as note" on failure, delete with inline confirm; first 3 shown, "Show N earlier attempts".
- [ ] Log a print form (mockup "log"): outcome segmented control (Success / Partial / Failed), prefill note, printer (`combo_box` autocomplete), date, material, brand and colour, layer, bed, hotend, nozzle, infill, supports, adhesion, print time, "What went wrong"/notes, photo drop zone; `Ctrl+Enter` saves; slide-in motion and the "Attempt N saved" toast from DESIGN.md "Motion".
- [ ] Commit `feat(app): recipe card and print log`.

### Task 9: Cards, list, smart filters and material chips

- [ ] Tests: `card_shows_chips_and_last_print`, `smart_filter_counts_in_sidebar`, `material_chip_filters`.
- [ ] Grid cards and list rows: recipe chips and last print (icon, word, date) as in the mockup; filter menu gains material chips (PLA, PETG, ABS, ASA, TPU) and status chips.
- [ ] Screenshots `docs/screenshots/m3-grid-*.png`, `m3-model-recipe-*.png`, `m3-log-form-*.png`.
- [ ] Commit `feat(app): recipe chips, last print and smart filters`.
