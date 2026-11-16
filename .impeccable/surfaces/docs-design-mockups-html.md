---
version: 1
slug: "docs-design-mockups-html"
primary_target: "docs/design/mockups.html"
related_targets: []
---

# Surface brief: Printdex app shell (design-language mockups)

Scope: the whole desktop app shell and its v1 screens (library, model view, log a print, settings), mocked in HTML to set the design language before iced code exists. Mode: Operate.

Audience and job: makers with 3D-printing libraries, from a small local folder to a multi-TB NAS. Find a model fast, see how it must be printed, log how the print went. Keyboard-first, light and dark.

Constraints: the mockup is reference only; the shipped UI is iced with a token theme. Everything must be buildable from iced's standard widget set plus custom containers (no web-only effects such as backdrop blur). No invented user counts or benchmarks; demo library data is synthetic.

## Direction contract

THESIS: Printdex is a natural-history collection for printable models: every model is a catalogued specimen with a label that tells you how it must be handled. Refuses the category default of a charcoal slicer-style shell with one orange or green accent and anonymous thumbnail tiles.

OWN-WORLD: Collection-cabinet green shell rail (deep grey-green enamel), cool archival-white ground in light and green-black in dark, brass for selection and focus like a cabinet's card holder (ring plus tint), specimen-label red reserved for failures only. Atkinson Hyperlegible Next for UI, Atkinson Hyperlegible Mono for measurements and catalogue numbers. 2px labels and chips, 4px controls and panels, 1px ink rules, no soft floating cards.

STORY: The user sees their library as a tidy collection, reads each model's needs straight off its label, opens it, checks the recipe, prints, and logs the attempt in seconds.

FIRST VIEWPORT: Left: green cabinet rail (libraries, collections, tags, smart filters, offline NAS state). Top: search field with filter chips and grid/list toggle. Main: virtualized thumbnail grid; each card is a render above a bordered specimen label (name, catalogue no., material chips, last-print status). Primary action "Log a print" lives in the model view header.

FORM: Specimen cabinet, position 6 of 7 on my ordered list. seed:361ee0d6e183525d. Signature move: the specimen label, a 1px-bordered caption block with catalogue number, set in mono, used on cards, the recipe card and print-log entries (attempt number in mono).

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance
