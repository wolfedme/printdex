# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

Printdex is a native desktop app (Rust + iced), not a website. It is recorded as `web` because its design language is custom and identical on Windows, macOS and Linux rather than following each OS's native look (confirmed by Max, 2026-11-16). Desktop window sizes only; there is no phone or tablet layout. Web-only tooling (HTML detector, live browser mode) does not apply.

## Stack

Rust, iced (pure-Rust, wgpu-based, Elm architecture). The design lands as a `theme` module in `printdex-app` implementing iced's theming with design tokens; widgets use tokens, never raw colors or sizes. Decided in `docs/superpowers/specs/2026-11-16-printdex-architecture-design.md`.

## Users

Hobbyists and makers with 3D-printing libraries, from a few hundred files on a local disk up to many thousands of files (multiple TB) on a NAS. Their job: find the model they want quickly, see how it needs to be printed, and record how each print actually went, without a server or cloud account.

## Product Purpose

Turn chaotic folders of STL/3MF/OBJ files into an organized, searchable library that also remembers how each model printed. Success: the library feels instant even on a slow NAS, and the user knows what settings a model needs before sending it to the slicer.

## Positioning

Existing tools (Manyfold, STL Shelf and others) have roughly the right features but fail on delivery: they need a self-hosted server, are tied to one OS, are unmaintained, or are slow and overcomplicated. Printdex is native on Windows, macOS and Linux, needs no server, is lightweight and fast on large NAS libraries, and speeds up the workflow instead of adding steps. It never uploads anything.

## Operating Context

- Libraries are folders on local disks, external drives or SMB/NAS shares; a folder with printable files is a model, and its files are parts.
- The user moves between Printdex, a slicer (opened from Printdex via "open with", never controlled by it), and the physical printer.
- Core loop: browse/search the library, open a model, check its recipe, print, then log the attempt (success / failed / partial, settings used, what went wrong, optional photo).
- Libraries can go offline (unmounted NAS) and must show as offline with data intact.

## Capabilities and Constraints

- v1 screens: library view (sidebar of libraries, collections, tags, smart filters; virtualized thumbnail grid with list mode; status and recipe badges on cards; instant search with filter chips), model view (cover and parts grid, 3D preview, recipe card, tags/collections, notes, source link, print-log timeline), "log a print" quick form, settings.
- Terminology: library, model, part, asset, tag, collection, recipe, print attempt / print log.
- Keyboard-first: global search shortcut, arrow-key grid navigation, quick tag entry.
- Light, dark and system theme.
- Performance targets: start to usable < 1 s, search per keystroke < 50 ms, scrolling never blocks on thumbnails, a 300 MB STL never freezes the UI.
- Must run without a capable GPU (software rendering fallback).
- Every user-visible string goes through Fluent translations; English only at launch.
- Never: slicing, printer control, cloud upload, accounts, telemetry, modifying model files, silent moving/renaming of files.
- Post-MVP: downloading from model sites, sync between machines.

## Brand Commitments

Name: Printdex. Tagline in the README: "Your STL library, organized. Local-first, blazing fast, built in Rust." No logo, voice guidelines or other brand commitments yet (Max, 2026-11-16).

## Evidence on Hand

No users, testimonials, benchmarks, screenshots or assets exist yet. Do not invent performance numbers, user counts or quotes. Open source under GPL-3.0.

## Product Principles

1. Speed is the feature: the app must feel instant, especially on large NAS libraries.
2. Get out of the way: every flow should save steps compared to browsing folders by hand.
3. The user's files are theirs: read, never modify; only explicit, confirmed, undoable file actions.
4. Local and private by default: nothing leaves the machine without the user asking.
5. Remember what the printer taught you: recipes and print logs are first-class, not an afterthought.

## Accessibility & Inclusion

Keyboard-first operation is required. No further accessibility requirements decided yet (Max, 2026-11-16).
