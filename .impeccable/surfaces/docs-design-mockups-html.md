---
version: 1
slug: "docs-design-mockups-html"
primary_target: "docs/design/mockups.html"
related_targets: []
---

# Surface brief: Printdex app shell (design-language mockups)

Scope: the whole desktop app shell and its v1 screens (library, model view, log a print, settings), mocked in HTML to set the design language before iced code exists. Mode: Operate.

Audience and job: makers with 3D-printing libraries, from a small local folder to a multi-TB NAS. Find a model fast, see how it must be printed, log how the print went. Keyboard-first, light and dark.

Constraints: the mockup is reference only; the shipped UI is iced with a token theme. Buildable from iced's standard widgets plus custom containers: solid borders only, no letter-spacing, no repeated gradients, app-drawn focus ring. Background blur comes only from the OS compositor behind a transparent window (Windows Mica/Acrylic, macOS vibrancy, KDE blur); iced has no in-app blur, and a shader blur would break the no-GPU requirement. Every glass surface needs a solid-tint fallback (GNOME, software rendering, reduced transparency). Demo data is synthetic.

History: first roll ran degraded and offered Specimen cabinet, Parts-drawer wall and the category standard. Max asked for previews of all, then re-rolled with the steer "I like translucent elements with blur behind them" and chose Frosted print on 2026-11-16 to iterate on. After comparing accents and seven full palettes, Max chose **Graphite signal** as the default and asked for the colour theme to be switchable later in Settings (the other six palettes ship as presets). Comparison page: docs/design/explorations.html.

## Direction contract

THESIS: Printdex is a shelf of frosted translucent prints: the interface is milky plastic panes, and every model shows up in the colour of the material it is printed in, so the library reads by material at a glance. Refuses the charcoal slicer-style shell with one orange accent and grey anonymous thumbnails.

OWN-WORLD: Frosted panes of neutral smoked glass (Graphite signal: near-neutral graphite tint; content panes ~70-86%, sidebar 82% so text holds over any wallpaper) with a bright 1px edge and 14px corners; one OS blur behind the window, panes differ only by fill alpha. Material colours for model previews, opaque (PETG teal, PLA orange, TPU magenta, ASA/unknown neutral). Signal yellow for selection, focus and the main action; red only for failure, amber for partial; recipe notes neutral. The colour theme (glass tint + accent) is a user setting with seven presets; material and status colours never change. Figtree for UI, JetBrains Mono for measurements and catalogue numbers.

STORY: The user scans a calm, glassy library where colour means material, opens a model, checks its recipe, prints, and logs the attempt in seconds.

FIRST VIEWPORT: Left: frosted sidebar (libraries, collections, smart filters, tags, offline NAS, scan progress). Top: frosted toolbar with search, filter chips, sort, grid/list toggle. Main: virtualized grid of frosted model panes, each a material-coloured preview, name with mono catalogue number, recipe chips, last-print status. Primary action "Log a print" (accent) in the model header.

MOTION: quick, physical and quiet. Focal moment is print-in (a finished preview builds up from the bed in 16 layers, 720ms, once per new thumbnail). Feedback: 90ms press, 4px hover lift of the model, springy toggles (only overshoot), sliding outcome indicator, model stays painted while the log panel slides, log entry slot-in with fading ring and toast. Colour, opacity, translate and clip only; reduced motion keeps fades.

FORM: Frosted print, my top-ranked candidate of the re-roll (Impeccable's pick), chosen by Max over the assigned light-table direction. seed:2c92208f21fd7a40. Signature move: material colour, the model preview tinted by its recipe material on cards, parts and the 3D viewer.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
