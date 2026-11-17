-- Catalogue numbers (№ 0412): per library, handed out in order and never
-- reused. `next_number` keeps counting even when models go away.

ALTER TABLE models ADD COLUMN number INTEGER;
ALTER TABLE libraries ADD COLUMN next_number INTEGER NOT NULL DEFAULT 1;

-- Existing models are numbered in the order Printdex first saw them.
UPDATE models SET number = (
  SELECT n FROM (
    SELECT id, row_number() OVER (PARTITION BY library_id ORDER BY created_at, rel_path) AS n
    FROM models
  ) ranked WHERE ranked.id = models.id
);
UPDATE libraries SET next_number = 1 + coalesce(
  (SELECT max(number) FROM models WHERE models.library_id = libraries.id), 0);

CREATE UNIQUE INDEX models_library_number ON models(library_id, number);
