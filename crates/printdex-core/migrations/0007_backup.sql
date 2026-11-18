-- Backup targets. `settings` is the target as JSON without secrets (those
-- live in the OS credential store). A list, though the app offers one
-- target for now.
CREATE TABLE backup_targets (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,                      -- folder | s3 | web-dav | sftp
  settings TEXT NOT NULL,
  encrypted INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);

-- Whether a library's model files are backed up. No row: not chosen yet,
-- which means off (the first model backup needs the user's confirm).
CREATE TABLE backup_libraries (
  library_id BLOB PRIMARY KEY REFERENCES libraries(id) ON DELETE CASCADE,
  models_on INTEGER NOT NULL
);

-- History of backup jobs. No foreign key on library_id: the history
-- outlives a removed library.
CREATE TABLE backup_runs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,                      -- data | models | prune | check
  library_id BLOB,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  result TEXT NOT NULL,                    -- running | ok | failed | cancelled | skipped
  bytes_added INTEGER NOT NULL DEFAULT 0,
  error TEXT
);
CREATE INDEX backup_runs_by_kind ON backup_runs (kind, library_id, started_at);

-- Install ids this install was restored from: their sidecars count as
-- this install's own.
CREATE TABLE install_predecessors (
  install_id TEXT PRIMARY KEY,
  added_at INTEGER NOT NULL
);
