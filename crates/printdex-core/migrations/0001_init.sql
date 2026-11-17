-- Printdex initial schema. Ids are UUID v7 (16-byte BLOBs); times are unix
-- milliseconds, except file mtimes (unix nanoseconds).

CREATE TABLE libraries (
  id BLOB PRIMARY KEY, path TEXT NOT NULL UNIQUE, display_name TEXT NOT NULL,
  kind TEXT NOT NULL DEFAULT 'local', sidecar_enabled INTEGER NOT NULL DEFAULT 1,
  last_full_scan_at INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE models (
  id BLOB PRIMARY KEY, library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
  missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE TABLE parts (
  id BLOB PRIMARY KEY, model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, format TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  fp_hash INTEGER, missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE INDEX parts_fp ON parts(size, fp_hash);
CREATE INDEX parts_model ON parts(model_id);
CREATE TABLE assets (
  id BLOB PRIMARY KEY, model_id BLOB NOT NULL REFERENCES models(id) ON DELETE CASCADE,
  library_id BLOB NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
  rel_path TEXT NOT NULL, size INTEGER NOT NULL, mtime_ns INTEGER NOT NULL,
  missing_since INTEGER, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
  UNIQUE (library_id, rel_path));
CREATE VIRTUAL TABLE models_fts USING fts5(
  name, rel_path, content='models', content_rowid='rowid',
  tokenize = "unicode61 remove_diacritics 2");
CREATE TRIGGER models_ai AFTER INSERT ON models BEGIN
  INSERT INTO models_fts(rowid, name, rel_path) VALUES (new.rowid, new.name, new.rel_path); END;
CREATE TRIGGER models_ad AFTER DELETE ON models BEGIN
  INSERT INTO models_fts(models_fts, rowid, name, rel_path) VALUES ('delete', old.rowid, old.name, old.rel_path); END;
CREATE TRIGGER models_au AFTER UPDATE OF name, rel_path ON models BEGIN
  INSERT INTO models_fts(models_fts, rowid, name, rel_path) VALUES ('delete', old.rowid, old.name, old.rel_path);
  INSERT INTO models_fts(rowid, name, rel_path) VALUES (new.rowid, new.name, new.rel_path); END;
