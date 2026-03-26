-- Site disable/enable toggle: allow disabling a site without deleting it
ALTER TABLE sites ADD COLUMN IF NOT EXISTS enabled BOOLEAN NOT NULL DEFAULT TRUE;
