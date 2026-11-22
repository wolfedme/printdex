-- Two-way sync (sync design §5, §12). Ids are UUID BLOBs, times unix ms.

CREATE TABLE sync_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

-- A sync space this install takes part in. Library spaces live inside a
-- library (`<root>/.printdex/sync`); target spaces on a folder or remote
-- target (`target` JSON, no secrets).
CREATE TABLE sync_spaces (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,                      -- library | target
  library_id BLOB REFERENCES libraries(id) ON DELETE CASCADE,
  target TEXT,
  name TEXT NOT NULL,
  space_id BLOB NOT NULL,
  encrypted INTEGER NOT NULL DEFAULT 0,
  -- Seq of the next batch this install writes.
  next_seq INTEGER NOT NULL DEFAULT 1,
  -- Seq covered by this install's newest snapshot, and records written since.
  snap_seq INTEGER NOT NULL DEFAULT 0,
  since_snap INTEGER NOT NULL DEFAULT 0,
  last_sync_at INTEGER,
  last_error TEXT,
  created_at INTEGER NOT NULL
);

-- A local library joined to a library of a space.
CREATE TABLE sync_links (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  lib_sync_id BLOB NOT NULL,
  PRIMARY KEY (space, library_id)
);
CREATE INDEX sync_links_lib ON sync_links(space, lib_sync_id);

-- Model sync ids and the local model each stands for. No local model:
-- parked (the model isn't on this computer). `alias_of`: two ids that
-- turned out to be the same model; the smaller one wins.
CREATE TABLE sync_models (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  lib_sync_id BLOB NOT NULL,
  sync_id BLOB NOT NULL,
  model_id BLOB REFERENCES models(id) ON DELETE SET NULL,
  alias_of BLOB,
  PRIMARY KEY (space, lib_sync_id, sync_id)
);
CREATE INDEX sync_models_model ON sync_models(model_id);

-- Collection sync ids and the local collection each stands for.
CREATE TABLE sync_collections (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  sync_id BLOB NOT NULL,
  collection_id BLOB REFERENCES collections(id) ON DELETE SET NULL,
  PRIMARY KEY (space, sync_id)
);

-- The winning stamp and value of every key this install knows, mine or
-- another install's. `seq` is the batch the stamp was published in;
-- `model` the model sync id the key belongs to.
CREATE TABLE sync_fields (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  model BLOB,
  stamp TEXT NOT NULL,
  seq INTEGER NOT NULL,
  hash INTEGER NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (space, key)
);
CREATE INDEX sync_fields_model ON sync_fields(space, model);

-- Models whose data changed since the last capture.
CREATE TABLE sync_dirty (model_id BLOB PRIMARY KEY);

-- How far this install has read each other install's log.
CREATE TABLE sync_cursors (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  install BLOB NOT NULL,
  seq INTEGER NOT NULL,
  PRIMARY KEY (space, install)
);

-- Other installs seen in a space.
CREATE TABLE sync_devices (
  space INTEGER NOT NULL REFERENCES sync_spaces(id) ON DELETE CASCADE,
  install BLOB NOT NULL,
  name TEXT NOT NULL,
  app_version TEXT NOT NULL DEFAULT '',
  last_seen INTEGER NOT NULL,
  PRIMARY KEY (space, install)
);

-- The losing side of a concurrent text edit ("Edited elsewhere").
CREATE TABLE edit_history (
  id BLOB PRIMARY KEY,
  model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  device TEXT NOT NULL,
  at INTEGER NOT NULL,
  dismissed INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX edit_history_model ON edit_history(model_id);
