# Printdex desktop app, English.
# Counts come in twice: $count (a number, for plural rules) and $n (the
# number already formatted with digit grouping, for display).

app-title = Printdex
app-opening = Opening your library…
app-open-failed = Printdex could not open its data folder at { $path }.

sidebar-libraries = Libraries
sidebar-all-models = All models
sidebar-add-library = Add library
sidebar-scanning = Scanning { $library }
sidebar-scan-listing = Listing files…
sidebar-scan-progress = { $done } of { $total } files

toolbar-search = Search models
toolbar-rescan = Rescan

pane-parts = { $count ->
    [one] 1 part
   *[other] { $n } parts
}

status-models = { $count ->
    [one] 1 model
   *[other] { $n } models
}
status-models-capped = Showing { $shown } of { $total } models
status-libraries = { $count ->
    [one] 1 library
   *[other] { $n } libraries
}
status-key-search = Search
status-key-move = Move
status-key-clear = Clear
status-key-open-model = Open
status-key-back = Back to library
status-key-part = Part
status-key-open-file = Open file
status-added = Added { $date }

model-show-in-folder = Show in folder
model-open = Open
model-parts = Parts
model-parts-summary = { $count ->
    [one] 1 file
   *[other] { $n } files
} · { $size }
model-other-files = Other files
model-details = Details
model-detail-library = Library
model-detail-folder = Folder
model-detail-parts = Parts
model-detail-size = Total size
model-detail-added = Added
model-detail-kind = Kind
model-kind-folder = Folder of files
model-kind-file = Single file
model-missing-title = This model is no longer in your library
model-missing-body = Its files were moved or deleted since the last scan.
model-back = Back to library

date-short = { $day } { $month } { $year }
month-1 = Jan
month-2 = Feb
month-3 = Mar
month-4 = Apr
month-5 = May
month-6 = Jun
month-7 = Jul
month-8 = Aug
month-9 = Sep
month-10 = Oct
month-11 = Nov
month-12 = Dec

empty-library-title = Add a folder with your models
empty-library-body = Printdex indexes your files where they are. Nothing is moved, renamed or uploaded.
empty-results-title = No models match “{ $query }”
empty-results-body = Try fewer or shorter words.
empty-filters-title = No models match these filters
empty-filters-body = Remove a filter to see more.

error-library-overlaps = That folder overlaps the library at { $path }.
error-library-invalid = That folder can’t be added. Pick a folder that exists and has a readable name.
error-library-offline = The library folder at { $path } can’t be reached.
error-already-scanning = This library is already being scanned.
error-other = Something went wrong. Details are in the log.
error-cache-clear = The preview cache couldn’t be cleared. Details are in the log.
error-settings-save = Your settings couldn’t be saved. Details are in the log.
error-launch = The file couldn’t be opened. Details are in the log.

list-name = Name
list-folder = Folder
list-parts = Parts
list-added = Added

filter-add = Filter
filter-format = Format
filter-parts = Parts
filter-single = Single file
filter-multi = Multi-part
sort-label = Sort
sort-name = Name
sort-recent = Recently added
header-models = { $count ->
    [one] 1 model
   *[other] { $n } models
}
header-match = { $shown } of { $total } models match

sidebar-settings = Settings

settings-title = Settings
settings-libraries = Libraries
settings-libraries-body = Folders Printdex catalogues. Model files are only read, never changed.
settings-col-library = Library
settings-col-location = Location
settings-col-status = Status
settings-col-models = Models
settings-status-ok = Up to date
settings-status-scanning = Scanning { $done } / { $total }
settings-status-listing = Scanning
settings-rescan = Rescan
settings-rename = Rename
settings-remove = Remove
settings-remove-confirm = Remove { $name } from Printdex? Your files stay where they are.
settings-remove-yes = Remove
settings-cancel = Cancel
settings-save = Save
settings-appearance = Appearance
settings-color-theme = Colour theme
settings-color-theme-body = Glass tint and accent. Status colours stay the same in every theme.
settings-mode = Theme
settings-mode-body = System follows your desktop’s light or dark setting.
settings-mode-system = System
settings-mode-light = Light
settings-mode-dark = Dark
settings-translucency = Translucency
settings-translucency-body = Frosted glass panes over a blurred desktop.
settings-translucency-unsupported = Your desktop can’t blur windows, so Printdex stays solid.
settings-translucency-restart = Takes effect the next time Printdex starts.
settings-translucency-auto = Automatic
settings-translucency-off = Off
settings-reduce-motion = Reduce motion
settings-reduce-motion-body = No lifting or scaling. Colour fades stay.
settings-about = About
settings-storage = Storage
settings-cache-limit = Preview cache size
settings-cache-limit-body = Thumbnails are kept on this computer. When the cache is full, the ones you haven’t seen for longest go first.
settings-cache = Preview cache
settings-cache-usage = { $used } of { $limit } used
settings-cache-measuring = Measuring…
settings-cache-clear = Clear cache
settings-version = Version
settings-data-folder = Data folder
settings-config-folder = Settings file
settings-licence = Licence
settings-licence-value = GPL-3.0. Printdex is free software.

theme-graphite = Graphite signal
theme-frost = Violet frost
theme-quartz = Smoked quartz
theme-seaglass = Sea glass
theme-resin = Amber resin
theme-rose = Rose PETG
theme-glacier = Glacier

sidebar-offline = Offline since { $time }
toolbar-retry = Retry
settings-status-offline = Offline since { $time }
settings-retry = Retry

## Previews
preview-rendering = Rendering preview
preview-too-large = Too large to preview
preview-unavailable = No preview
viewer-loading = Loading 3D preview
viewer-reset = Reset view
viewer-full-detail = Full detail
viewer-simplified = 3D preview simplified to { $shown }
viewer-unavailable = No 3D preview
status-key-turn = Drag to turn, Shift-drag to move, scroll to zoom
model-measured = { $x } × { $y } × { $z } mm
model-triangles = { $count ->
    [one] { $n } triangle
   *[other] { $n } triangles
}

## Organizing
sidebar-collections = Collections
sidebar-new-collection = Collection name
sidebar-collections-empty = No collections yet
sidebar-smart = Smart filters
sidebar-tags = Tags
sidebar-tags-empty = No tags yet
sidebar-tags-all = Show all { $count } tags
sidebar-tags-fewer = Show fewer tags
smart-never-printed = Never printed
smart-failed-last = Failed last time
smart-added-week = Added this week
header-tag = Tagged “{ $tag }”
sort-collection = Collection order
collection-rename = Rename collection
collection-delete = Delete collection
collection-delete-confirm = Delete this collection? Its models stay in your library.
collection-new = New collection…
collection-add-model = Add to collection
collection-remove-model = Remove from collection
action-save = Save
action-cancel = Cancel
model-tags = Tags
model-collections = Collections
model-source = Source
model-notes = Notes
tag-add = Tag
tag-remove = Remove tag
tag-placeholder = Add a tag
tag-hint = Enter adds the tag. Esc closes.
source-add = Add source link
source-edit = Edit source link
source-placeholder = https://…
notes-empty = No notes yet. Press N to write some.
notes-placeholder = What to remember about this model. Markdown works.
notes-edit = Edit notes
notes-key-save = Save
notes-key-cancel = Cancel
status-key-tag = Tag
status-key-notes = Notes
settings-sidecars = Sidecar files
settings-sidecars-body = Printdex can keep a small .printdex.toml file next to each model you tag, note or log. It holds what you wrote, so it travels with the folder and survives reinstalling. Printdex never writes any other file into a library.
settings-sidecars-on = Writes .printdex.toml next to models you organize
settings-sidecars-off = Writes nothing into this library
notice-sidecar-foreign = { $library } has notes and tags from another Printdex install. They were added to this one.
error-source-invalid = That link can’t be used. Use a web address starting with http:// or https://.
error-collection-exists = A collection with that name already exists.
error-collection-name = Collection names can’t be empty or longer than 80 characters.
error-tag-invalid = Tags can’t be empty or longer than 64 characters.
error-text-too-long = That text is too long to save.
action-close = Close
status-key-log = Log a print
outcome-success = Success
outcome-partial = Partial
outcome-failed = Failed
supports-none = None
supports-normal = Normal
supports-tree = Tree
adhesion-none = None
adhesion-skirt = Skirt
adhesion-brim = Brim
adhesion-raft = Raft
recipe-title = Recipe
recipe-empty = No recipe yet. Write down the settings that work, or save one from a successful print.
recipe-write = Write recipe
recipe-edit = Edit recipe
recipe-from = from attempt { $n } · { $date }
recipe-required = Must match: { $fields }
recipe-orientation = Orientation
log-title = Print log
log-count = { $count ->
    [one] { $n } attempt
   *[other] { $n } attempts
}
log-a-print = Log a print
log-edit-title = Edit attempt
log-empty = Nothing printed yet. Log a print to remember what worked and what didn’t.
log-attempt = Attempt { $n }
log-show-earlier = { $count ->
    [one] Show { $n } earlier attempt
   *[other] Show { $n } earlier attempts
}
log-show-fewer = Show fewer
log-recipe-from-this = The recipe comes from this attempt.
log-save-recipe = Save as recipe
log-add-as-note = Add as note
log-add-note = Add note
log-note-placeholder = What to remember next time
log-note-hint = Added to the recipe notes for { $material ->
    [none] every material
   *[other] { $material }
}.
log-edit = Edit attempt
log-delete = Delete attempt
log-delete-confirm = Delete this attempt and its photos?
log-saved = Attempt { $n } saved
form-outcome = Outcome
form-prefilled = Filled in from the recipe. Change what you did differently.
form-printer = Printer
form-date = Date
form-material = Material
form-filament = Filament
form-layer = Layer
form-nozzle = Nozzle
form-infill = Infill
form-bed = Bed
form-hotend = Hotend
form-walls = Walls
form-supports = Supports
form-adhesion = Adhesion
form-unset = Not set
form-duration = Print time
form-notes = Notes
form-what-went-wrong = What went wrong
form-photo-add = Add photos, or drop them here
form-photo-pick = Choose photos
form-photo-filter = Images
form-required = Must match next time
form-recipe-notes = Notes
form-new-note = Add a note and press Enter
form-remove-note = Remove note
form-save-attempt = Save attempt
form-key-save = Save
form-error-range = { $field } must be between { $min } and { $max }.
form-error-number = { $field } must be a number.
form-error-duration = Write the print time like 2h 15m, or in minutes.
form-error-date = Use a date like 2026-11-17, today or earlier.
form-error-photo = That photo can’t be added. Use PNG, JPEG or WebP.
card-supports = Supports
card-never-printed = Never printed
card-printed = Printed { $n }×
list-last-print = Last print
filter-material = Material
