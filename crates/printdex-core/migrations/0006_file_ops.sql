-- Journal of file actions (rename, move, trash). A row is written before
-- the files are touched and updated after, so a crash in between can be
-- reconciled with the disk on the next start.
CREATE TABLE file_ops (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  kind TEXT NOT NULL,                      -- rename | move | trash
  state TEXT NOT NULL,                     -- planned | done | undone | failed
  at INTEGER NOT NULL
);

CREATE TABLE file_op_steps (
  op_id INTEGER NOT NULL REFERENCES file_ops(id) ON DELETE CASCADE,
  seq INTEGER NOT NULL,
  model_id BLOB NOT NULL,
  model_kind TEXT NOT NULL,                -- folder | file
  name TEXT NOT NULL,                      -- the model's name before
  new_name TEXT NOT NULL,                  -- and after
  from_lib BLOB NOT NULL,
  from_rel TEXT NOT NULL,
  to_lib BLOB NOT NULL,
  to_rel TEXT NOT NULL,
  how TEXT NOT NULL,                       -- rename | copy | trash
  PRIMARY KEY (op_id, seq)
);
