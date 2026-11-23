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
scan-done-first = { $count ->
    [0] No models found in { $library } yet. Printdex looks for STL, 3MF and OBJ files.
    [one] Found 1 model in { $library } in { $time }
   *[other] Found { $n } models in { $library } in { $time }
}
scan-done-same = { $library } is up to date. Checked { $count ->
    [one] 1 file
   *[other] { $n } files
} in { $time }.
scan-done-changed = { $library } rescanned in { $time }: { $changes }
scan-change-new = { $n } new
scan-change-moved = { $count ->
    [one] 1 moved with everything you wrote
   *[other] { $n } moved with everything you wrote
}
scan-change-back = { $n } back
scan-change-missing = { $n } missing

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

model-show-in-folder = Show in folder
model-open = Open
model-open-in = Open in { $app }
model-open-default = Open with default app
model-open-choose = Choose app for { $format } files…
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
model-missing-since = Missing since { $date }. Its files were moved or deleted outside Printdex. What you wrote about it is kept.
model-where-it-was = Show where it was
model-forget = Forget…
model-forget-confirm = Forget { $name }? Its tags, notes, recipe and print log are deleted. No files are touched.
model-forget-yes = Forget model
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
empty-clear-search = Clear search
empty-clear-filters = Clear filters
empty-failed-last-title = Nothing failed last time
empty-failed-last-body = No model’s latest print failed. When one does, it waits here until you log a print that works.
empty-never-printed-title = Every model here has been printed
empty-never-printed-body = Models you haven’t logged a print for show up here.
empty-missing-title = Nothing is missing
empty-missing-body = Every model is where Printdex last saw it.
empty-added-week-title = Nothing new this week
empty-added-week-body = Models first found in the last seven days show up here.
empty-collection-title = This collection is empty
empty-collection-body = Open a model and choose Add to collection to put it here.
empty-tag-title = No models have this tag
empty-tag-body = Open a model and press T to tag it.
empty-folder-title = No models in this folder yet
empty-folder-body = Printdex looks for STL, 3MF and OBJ files. Add some to the folder, then press Rescan.

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
settings-apps = Apps
settings-apps-body = Open models straight in your slicer. Pick an app per file type; Printdex hands it the files and nothing else.
settings-apps-format = { $format } files
settings-apps-default = The system's default app
settings-apps-chosen = { $app } · { $path }
settings-apps-choose = Choose…
settings-apps-reset = Use default
apps-choose = Choose an app
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
smart-missing = Missing
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
settings-changes = Change detection
settings-changes-body = How Printdex notices files you add, move or delete outside the app. Local folders are watched; network shares are checked on a timer, because watching rarely works over the network.
change-auto = Automatic
change-watch = Watch for changes
change-every = Check every { $minutes } minutes
change-manual = Only when I refresh
kind-local = Local folder
kind-network = Network share
kind-removable = Removable drive
change-now-watch = { $kind }, watched for changes
change-now-every = { $kind }, checked every { $minutes } minutes
change-now-manual = { $kind }, checked only when you refresh
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
recipe-applies-to = Settings for
recipe-whole-model = Whole model
recipe-parts-caption = Pick a part to give it settings that differ from the rest.
recipe-part-caption = Only what differs for { $part }. Empty fields use the whole model's value.
recipe-part-clear = Same as whole model
log-title = Print log
log-count = { $count ->
    [one] { $n } attempt
   *[other] { $n } attempts
}
log-count-time = { $attempts } · { $time } logged
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
log-saved-with = { $saved }. { $extra }
log-saved-first-try = Printed on the first try.
log-saved-first-success = { $count ->
    [one] First success, after 1 try that didn’t work.
   *[other] First success, after { $n } tries that didn’t work.
}
log-saved-note-hint = “Add as note” on it keeps the fix in the recipe.
form-outcome = Outcome
form-outcome-keys = S, P or F
form-settings-folded = Same settings as the recipe
form-settings-change = Change
form-duration-hint = 2h 15m
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
form-as-model = As model
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
form-error-outcome = Pick how it went: Success, Partial or Failed.
form-error-duration = Write the print time like 2h 15m, or in minutes.
form-error-date = Use a date like 2026-11-17, today or earlier.
form-error-photo = That photo can’t be added. Use PNG, JPEG or WebP.
card-supports = Supports
card-printed = Printed { $n }×
list-last-print = Last print
filter-material = Material

## File actions
files-rename = Rename
files-move = Move to…
files-trash = Move to trash
files-clear = Clear selection
files-selected = { $count ->
    [one] 1 model selected
   *[other] { $count } models selected
}
files-set-material = Set material
files-clear-material = No material
files-material-set = { $count ->
    [one] { $material } set on 1 model
   *[other] { $material } set on { $count } models
}
files-material-cleared = { $count ->
    [one] Material cleared on 1 model
   *[other] Material cleared on { $count } models
}
files-rename-title = Rename model
files-rename-yes = Rename
files-move-pick = Choose a folder in one of your libraries
files-move-where = { $folder } in { $library }
files-move-title = { $count ->
    [one] Move 1 model to { $where }?
   *[other] Move { $count } models to { $where }?
}
files-move-yes = Move
files-trash-title = { $count ->
    [one] Move 1 model to the trash?
   *[other] Move { $count } models to the trash?
}
files-trash-body = The files go to the system trash. Models you tagged, noted or printed stay under Missing with everything you wrote; the rest are forgotten.
files-trash-yes = Move to trash
files-and-more = { $count ->
    [one] , and 1 more
   *[other] , and { $count } more
}
files-renamed = Renamed to { $name }
files-moved = { $count ->
    [one] Moved { $name }
   *[other] Moved { $count } models
}
files-trashed = { $count ->
    [one] Moved { $name } to the trash
   *[other] Moved { $count } models to the trash
}
files-trashed-finder = { $count ->
    [one] Moved { $name } to the trash. Restore it from the Trash in Finder.
   *[other] Moved { $count } models to the trash. Restore them from the Trash in Finder.
}
files-undone = Undone
files-undo = Undo
files-error-name = That name can't be used. Avoid / \ : * ? " < > | and names starting with a dot.
files-error-taken = Something named { $name } is already there.
files-error-offline = The destination folder can't be reached right now.
files-error-source-offline = The model's library can't be reached right now.
files-error-busy = The library is being scanned. Try again in a moment.
files-error-trash = The system trash can't be used for this folder.
files-error-undo = There is nothing to undo.
files-error-gone = The model's files are no longer where Printdex saw them.
files-error-outside = That folder is not in one of your libraries.
files-error-other = The files could not be changed. Nothing was lost; see the log for details.

## Updates
updates-ask = Check for updates automatically? Printdex looks at its GitHub releases once a day and sends nothing about you or your library.
updates-yes = Check automatically
updates-not-now = Not now
updates-title = Updates
updates-auto = Check automatically
updates-auto-hint = Once a day, from Printdex's GitHub releases. Updates install only when you restart.
updates-check-now = Check now
updates-idle = Not checked yet.
updates-checking = Checking for updates…
updates-up-to-date = Printdex is up to date.
updates-downloading = Downloading Printdex { $version }…
updates-ready = Printdex { $version } is ready to install.
updates-restart = Restart to update
updates-failed = Could not check for updates: { $error }
updates-unavailable = Updates come with the installed app. This copy wasn't installed with the Printdex installer, so it doesn't update itself.

## Backup (Settings › Backup and the sidebar line)
settings-backup = Backup
backup-body = Keep a copy of your library somewhere safe. Printdex backs up its own data (tags, collections, recipes, print log, photos and settings) on its own, and the model files of the libraries you pick.
backup-loading = Loading…
backup-dismiss = Dismiss
backup-empty-title = No backup yet
backup-empty-body = Pick a folder on another drive, an S3 bucket, a WebDAV server or an SFTP server. Snapshots are deduplicated and compressed, so later backups only send what changed.
backup-set-up = Set up backup
backup-where = Where backups go
backup-where-body = A folder can be a USB drive or a mounted network share.
backup-kind-folder = Folder
backup-kind-s3 = S3
backup-kind-webdav = WebDAV
backup-kind-sftp = SFTP
backup-field-folder = Folder
backup-field-endpoint = Endpoint
backup-field-bucket = Bucket
backup-field-region = Region
backup-field-prefix = Folder in bucket
backup-field-key-id = Access key ID
backup-field-secret-key = Secret access key
backup-field-url = WebDAV address
backup-field-user = User name
backup-field-password = Password
backup-field-host = Server
backup-field-port = Port
backup-field-remote-folder = Folder on server
backup-field-key-file = Private key
backup-key-agent = Uses your ssh agent when empty
backup-field-secret = Password or secret key
backup-field-passphrase = Passphrase
backup-field-passphrase2 = Repeat passphrase
backup-choose = Choose…
backup-pick-folder = Choose a backup folder
backup-pick-key = Choose a private key
backup-test = Test connection
backup-testing = Testing…
backup-test-ok = Connected. Printdex can write here.
backup-test-existing = Connected. A Printdex backup is already here and the passphrase opens it; new snapshots join it.
backup-encrypt = Encrypt backups
backup-encrypt-body-folder = Off is fine for a drive you keep at home. Turn it on if the drive could be lost or shared.
backup-encrypt-body-remote = Recommended: the server only ever sees encrypted data.
backup-ack = I understand that a lost passphrase can’t be recovered, and neither can the backup.
backup-remember = Remember on this computer
backup-no-keyring = This computer has no credential store, so Printdex asks again after a restart.
backup-not-remembered = The passphrase couldn’t be stored on this computer. Printdex will ask for it after a restart.
backup-models = Model files
backup-models-body = Model files can be large. Network shares are often backed up by the NAS itself, so they start off.
backup-models-none = Add a library first.
backup-estimate = About { $size }
backup-models-confirm = The first backup uploads about { $size }. Later ones only send changes.
backup-models-yes = Back up models
backup-need-fields = Fill in where backups go.
backup-need-secret = Enter the password or secret key.
backup-need-passphrase = Choose a passphrase.
backup-passphrase-mismatch = The passphrases don’t match.
backup-need-ack = Confirm that you understand a lost passphrase can’t be recovered.
backup-save = Save
backup-saving = Checking…
backup-back = Back
backup-confirm-title = Back up model files now?
backup-confirm-body = About { $size } from { $count ->
    [one] one library
   *[other] { $count } libraries
} will be uploaded to { $target }. Later backups only send changes.
backup-confirm-yes = Save and back up
backup-encrypted = Encrypted
backup-not-encrypted = Not encrypted
backup-size = Uses { $size }
backup-last = Last backup { $time }
backup-never = Not backed up yet.
backup-now = Back up now
backup-running = Backing up { $what }…
backup-cancel = Cancel
backup-cancelled = The backup was cancelled.
backup-remove = Stop backing up here
backup-remove-confirm = Stop backing up to this target? The backups already there stay.
backup-remove-yes = Stop
backup-removed-library = Removed library
backup-schedule = Schedule
backup-schedule-body = Printdex data is backed up a few minutes after you change something, and at least once a day.
backup-schedule-models = Model files
backup-schedule-models-body = Manual backs up models only on “Back up now”.
backup-daily = Daily
backup-weekly = Weekly
backup-manual = Manual
backup-limit = Upload limit
backup-limit-body = Keeps backups from filling your connection.
backup-limit-off = No limit
backup-history = History
backup-history-body = The last 50 backup jobs.
backup-history-empty = Nothing has run yet.
backup-run-data = Printdex data
backup-run-models = Models of { $library }
backup-run-prune = Clean-up
backup-run-check = Check
backup-result-ok = Done
backup-result-running = Running
backup-result-cancelled = Cancelled
backup-result-skipped = Skipped
backup-result-failed = Failed
backup-prompt-title = Printdex needs your backup passphrase
backup-prompt-body = It isn’t stored on this computer, so backups are paused until you enter it.
backup-unlock = Continue
backup-status-idle = Backup ready
backup-status-starting = Backing up…
backup-status-running = Backing up { $percent } %
backup-status-last = Last backup { $time }
backup-status-attention = Backup needs attention
backup-attn-data-stale = Printdex data hasn’t been backed up for 3 days.
backup-attn-models-stale = Models of { $library } haven’t been backed up for 14 days.
backup-attn-connect = Connect { $library }: its models haven’t been backed up for 14 days because it was offline.
backup-attn-unreachable = The backup target can’t be reached. Printdex keeps trying.
backup-attn-full = The backup target is full.
backup-attn-credentials = The backup target rejected the password or key.
backup-attn-passphrase = Enter the backup passphrase to continue.
backup-attn-check = The last check found problems in the backup. Details are in the history.
backup-error-passphrase = That passphrase doesn’t open the backup.
backup-error-credentials = The target rejected the password or key.
backup-error-unreachable = The target can’t be reached. Check the address and your connection.
backup-error-full = The target is full.
backup-error-read-only = Printdex can’t write to the target.
backup-error-invalid = Those target settings can’t be used.
backup-error-unsupported = This kind of target isn’t available on this system.
backup-error-not-configured = No backup target is set up.
backup-error-secrets = Enter the backup passphrase first.
backup-error-no-repository = There is no Printdex backup at that target.
backup-error-snapshot = That snapshot no longer exists.
backup-error-check = The check found problems in the backup.

## Restore
restore-start = Restore from backup
restore-title = Restore from backup
restore-body = Where are your backups? Printdex lists the points in time it finds there.
restore-encrypted = The backup is encrypted
restore-encrypted-body = Turn on if you set a passphrase when you set up backups.
restore-find = Find backups
restore-finding = Looking…
restore-none-found = No Printdex backups were found there.
restore-pick-title = Pick a point in time
restore-pick-body = Newest first. Your tags, collections, recipes, print log, photos and settings come back as they were then.
restore-this-computer = This computer
restore-other-computer = Another computer
restore-snapshot-size = { $files ->
    [one] one file
   *[other] { $files } files
} · { $size }
restore-replaces = This replaces Printdex’s data on this computer. The current data is kept aside until the next backup.
restore-other-warning = This backup came from another computer.
restore-go = Restore
restore-restoring = Restoring…
restore-done-title = Restored
restore-done-body = Printdex restarts to finish. Afterwards it asks where each library lives now.
restore-restart = Restart Printdex
restore-where = Where is { $library } now?
restore-where-body = It was at { $path }.
restore-where-pick = Where is { $library } now?
restore-relinking = Scanning…
restore-here = It’s already here
restore-files = Restore files from backup
restore-files-pick = Restore the files of { $library } into…
restore-skip = Skip for now
restore-model = Restore from backup…
restore-model-title = Restore from backup
restore-close = Close
restore-model-loading = Looking for backups of this model…
restore-model-none = No backup holds this model yet.
restore-model-comparing = Comparing with the files here…
restore-model-same = The files here match this backup.
restore-file-changed = Changed
restore-file-missing = Missing
restore-model-trash = Files that differ go to the trash first; nothing is overwritten.
restore-model-done = { $count ->
    [one] Restored one file.
   *[other] Restored { $count } files.
}
restore-model-here = Restore here
restore-model-elsewhere = Restore to another folder…
restore-model-pick = Restore a copy into…

## Sync

settings-sync = Sync
sync-body = Keep tags, collections, notes, recipes and the print log the same on all your computers. Model files are never copied: share them with your NAS, Syncthing or Dropbox.
sync-loading = Loading…
sync-on = Sync
sync-on-body = Off keeps your spaces and your data. Changes made meanwhile go out when you turn it back on.
sync-device = This device
sync-device-body = How other computers name this one.
sync-device-save = Save
sync-spaces = Spaces
sync-spaces-body = A space is the folder where your computers leave their changes for each other.
sync-empty-title = Not syncing yet
sync-empty-body = Put a space inside a library that every computer opens from your NAS, or in another place (a shared folder, S3, WebDAV or SFTP) when each computer has its own copy of the files.
sync-add = Add space
sync-now = Sync now
sync-syncing = Syncing…
sync-kind-library = Inside a library
sync-kind-target = Another place
sync-kind-library-body = Every computer opens this library from the same NAS share. The space sits in its .printdex folder.
sync-kind-target-body = Each computer has its own copy of the files. The space can be a shared folder, an S3 bucket, a WebDAV server or an SFTP server.
sync-where = Where the space goes
sync-library = Library
sync-library-first = Library to sync first
sync-need-library = Pick a library.
sync-need-fields = Fill in where the space goes.
sync-ack = I understand that a lost passphrase can’t be recovered: this space’s data can only be read with it.
sync-pick-folder = Choose a folder for the space
sync-encrypt = Encrypt the space
sync-encrypt-body-library = Off is fine on your own NAS. Every computer needs the passphrase once.
sync-encrypt-body-target = Recommended: the place only ever sees encrypted data. Every computer needs the passphrase once.
sync-joining = This space is encrypted. Enter the passphrase it was set up with.
sync-save = Add space
sync-saving = Adding…
sync-kind-label-library = library space
sync-encrypted = Encrypted
sync-locked = Locked
sync-unlock = Enter passphrase
sync-unlock-body = This computer doesn’t have the passphrase of { $space }.
sync-unlock-confirm = Unlock
sync-last = Synced { $ago }
sync-last-never = Not synced yet
sync-last-error = Last try failed: { $error }
sync-libraries = Libraries
sync-libraries-none = No library syncs through this space yet.
sync-library-pair = { $local } ↔ { $remote }
sync-link = Link a library
sync-unlink = Stop syncing
sync-devices = Devices
sync-device-this = { $name } (this device)
sync-device-seen = { $name } · seen { $ago }
sync-forget = Forget
sync-forget-hint = A device can be forgotten after 30 days without syncing.
sync-forget-confirm = Forget { $name }? Its changes stay; its folder in the space is removed.
sync-forget-yes = Forget device
sync-parked = { $count ->
    [one] 1 model from another computer isn’t on this one
   *[other] { $n } models from other computers aren’t on this one
}
sync-parked-show = Show
sync-parked-hide = Hide
sync-parked-body = Their data waits here and arrives as soon as a scan finds them.
sync-parked-from = from { $device }
sync-remove = Leave space
sync-remove-confirm = Leave { $space }? This computer stops syncing through it. Your data here stays as it is.
sync-remove-yes = Leave space
sync-link-title = Link a library
sync-link-body = Pick the library on this computer and the one in the space it matches.
sync-link-local = On this computer
sync-link-remote = In the space
sync-link-new = Add it as a new library
sync-link-suggested = same models
sync-link-from = from { $device }
sync-link-confirm = Link
sync-link-linking = Linking…
sync-link-none = Every library here already syncs through this space.
sync-this-library = Sync this library…
sync-status-off = Sync off
sync-status-locked = Passphrase needed
sync-status-waiting = Waiting for { $space }
sync-status-synced = Synced { $ago }
sync-status-never = Not synced yet
sync-status-syncing = Syncing…
sync-pop-queued = { $count ->
    [0] Nothing waiting to go out
    [one] 1 changed model waiting to go out
   *[other] { $n } changed models waiting to go out
}
sync-pop-settings = Sync settings
sync-on-device = on { $device }
sync-edited-title = Edited elsewhere
sync-edited-body = These edits were made at the same time on two computers. The newer one was kept; here is the other.
sync-edited-notes = Notes
sync-edited-recipe = Recipe notes
sync-edited-attempt = Print notes
sync-edited-other = Text
sync-edited-meta = { $field } · on { $device }, { $time }
sync-edited-restore = Restore
sync-edited-copy = Copy
sync-edited-dismiss = Dismiss
sync-edited-copied = Copied
sync-error-passphrase = That passphrase doesn’t open this space.
sync-error-passphrase-needed = This space is encrypted: enter its passphrase.
sync-error-unreachable = The space can’t be reached right now. Printdex tries again on its own.
sync-error-read-only = Printdex can’t write to the space. Check that this computer may write there.
sync-error-newer = The space was set up by a newer Printdex. Update Printdex on this computer.
sync-error-already-linked = This library already syncs through that space.
sync-error-device-recent = That device synced too recently to be forgotten.
sync-error-damaged = A file in the space is damaged. Printdex skips it and keeps the rest.
ago-now = just now
ago-min = { $n } min ago
ago-hours = { $n } h ago
ago-days = { $n ->
    [one] 1 day ago
   *[other] { $n } days ago
}
