-- How Printdex notices changes in a library: 'auto' (watch local folders,
-- poll shares every 15 minutes), 'watch', 'every:<minutes>' or 'manual'.
ALTER TABLE libraries ADD COLUMN change_mode TEXT NOT NULL DEFAULT 'auto';
