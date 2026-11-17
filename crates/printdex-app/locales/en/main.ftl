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

error-library-overlaps = That folder overlaps the library at { $path }.
error-library-invalid = That folder can’t be added. Pick a folder that exists and has a readable name.
error-library-offline = The library folder at { $path } can’t be reached.
error-already-scanning = This library is already being scanned.
error-other = Something went wrong. Details are in the log.
error-settings-save = Your settings couldn’t be saved. Details are in the log.
error-launch = The file couldn’t be opened. Details are in the log.
