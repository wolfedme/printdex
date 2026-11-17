-- Part measurements from parsing the mesh (milestone 2). They belong to the
-- fingerprint in measured_fp; a changed file is measured again.
ALTER TABLE parts ADD COLUMN dim_x REAL;
ALTER TABLE parts ADD COLUMN dim_y REAL;
ALTER TABLE parts ADD COLUMN dim_z REAL;
ALTER TABLE parts ADD COLUMN triangles INTEGER;
ALTER TABLE parts ADD COLUMN measured_fp INTEGER;
