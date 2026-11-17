-- What the user knows about a model (milestone 3): notes, source, tags,
-- collections, the recipe and the print log. Ids are UUID v7 BLOBs, times
-- unix milliseconds. Settings are JSON (see model_data.rs).

CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);

ALTER TABLE models ADD COLUMN notes TEXT NOT NULL DEFAULT '';
ALTER TABLE models ADD COLUMN source_url TEXT;
-- Last change to notes, source, tags or collections (newest wins on import).
ALTER TABLE models ADD COLUMN data_updated_at INTEGER;
-- The sidecar's mtime (ns) when Printdex last wrote or imported it.
ALTER TABLE models ADD COLUMN sidecar_mtime_ns INTEGER;

CREATE TABLE tags (
  id BLOB PRIMARY KEY, name TEXT NOT NULL, name_key TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE model_tags (
  model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  tag_id BLOB NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (model_id, tag_id));
CREATE INDEX model_tags_tag ON model_tags(tag_id);

CREATE TABLE collections (
  id BLOB PRIMARY KEY, name TEXT NOT NULL, name_key TEXT NOT NULL UNIQUE,
  created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE collection_models (
  collection_id BLOB NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
  model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  position INTEGER NOT NULL, created_at INTEGER NOT NULL,
  PRIMARY KEY (collection_id, model_id));
CREATE INDEX collection_models_model ON collection_models(model_id);

CREATE TABLE recipes (
  model_id BLOB PRIMARY KEY REFERENCES models(id) ON DELETE CASCADE,
  material TEXT, settings TEXT NOT NULL DEFAULT '{}', required TEXT NOT NULL DEFAULT '[]',
  notes TEXT NOT NULL DEFAULT '[]', parts TEXT NOT NULL DEFAULT '[]',
  from_attempt BLOB, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE INDEX recipes_material ON recipes(material);

CREATE TABLE attempts (
  id BLOB PRIMARY KEY, model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  printed_at INTEGER NOT NULL, result TEXT NOT NULL, material TEXT,
  settings TEXT NOT NULL DEFAULT '{}', duration_min INTEGER, notes TEXT NOT NULL DEFAULT '',
  parts TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE INDEX attempts_model ON attempts(model_id, printed_at);

CREATE TABLE attempt_photos (
  id BLOB PRIMARY KEY, attempt_id BLOB NOT NULL REFERENCES attempts(id) ON DELETE CASCADE,
  file_name TEXT NOT NULL, created_at INTEGER NOT NULL);
CREATE INDEX attempt_photos_attempt ON attempt_photos(attempt_id);

-- Search over name and path (kept by triggers) plus tags, notes and recipe
-- text (kept by the engine). A regular FTS5 table so the engine can update
-- single columns.
DROP TRIGGER models_ai;
DROP TRIGGER models_ad;
DROP TRIGGER models_au;
DROP TABLE models_fts;
CREATE VIRTUAL TABLE models_fts USING fts5(
  name, rel_path, tags, notes, recipe,
  tokenize = "unicode61 remove_diacritics 2");
INSERT INTO models_fts(rowid, name, rel_path, tags, notes, recipe)
  SELECT rowid, name, rel_path, '', '', '' FROM models;
CREATE TRIGGER models_ai AFTER INSERT ON models BEGIN
  INSERT INTO models_fts(rowid, name, rel_path, tags, notes, recipe)
  VALUES (new.rowid, new.name, new.rel_path, '', new.notes, ''); END;
CREATE TRIGGER models_ad AFTER DELETE ON models BEGIN
  DELETE FROM models_fts WHERE rowid = old.rowid; END;
CREATE TRIGGER models_au AFTER UPDATE OF name, rel_path ON models BEGIN
  UPDATE models_fts SET name = new.name, rel_path = new.rel_path WHERE rowid = new.rowid; END;
