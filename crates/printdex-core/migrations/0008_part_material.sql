-- Material inferred for a part (issue #128), used when the model has no
-- recipe or print material: what the 3MF's slicer settings name
-- (file_material, read at the fingerprint in material_fp), else a material
-- word in the path (path_material: '' for none, NULL until looked at).
ALTER TABLE parts ADD COLUMN file_material TEXT;
ALTER TABLE parts ADD COLUMN material_fp INTEGER;
ALTER TABLE parts ADD COLUMN path_material TEXT;
