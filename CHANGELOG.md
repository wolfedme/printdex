# Changelog

All notable changes to Printdex. Versions follow [Semantic Versioning](https://semver.org).

## Unreleased

### Features

- **core:** Model detail, library management, filters (#38)
- **app:** All colour themes, glass and solid sets
- **app:** Settings file and screen routing
- **app:** Model view
- **app:** List mode
- **app:** Filter chips and sort
- **app:** Settings screen
- **app:** Offline library state
- **app:** Motion
- **mesh:** STL, 3MF and OBJ parsers
- **render:** CPU thumbnail renderer
- **core:** Thumbnail queue and cache
- **app:** Real previews in grid, list and model view
- **render:** GPU thumbnails with CPU fallback
- **viewer:** Interactive 3D preview with GPU drawing and CPU fallback
- **app:** Print-in, storage settings, viewer polish
- **core:** User data schema, tags, collections, recipes, attempts and scoped search
- **core:** Sidecars, photos, engine commands and material-coloured previews
- **app:** Tags, collections, notes and source
- Catalogue numbers per library (№ 0412)
- **app:** Recipe card, print log and Log a print
- **core:** Fs abstraction, chaos wrapper, scanner hardening (#25)
- **core:** Keep model identity across moves and renames
- **core:** Missing grace period and Missing filter
- **core:** Library monitor with watching, timers and offline detection
- **app:** Change detection setting and Missing model view
- **core:** Rename, move and trash with undo journal
- **app:** File actions with undo, multi-select and Open with

### Fixes

- Use as_chunks for fixed-size chunks (clippy 1.100)

### Performance

- Bench budgets, baseline check and scoped folder rescans

