-- Backup-encryption key derivation versioning (s434).
--
-- derive_backup_encryption_key used to be a single, unversioned function —
-- fine while there was only one derivation, but the derivation itself was
-- weak (a literal JWT_SECRET substring: format!("backup-enc-{}", &secret[..32])),
-- so fixing it means an install with existing encrypted backups needs restore
-- to reproduce whichever derivation actually wrote a given file, not just
-- "the current one". Every row that predates this column was necessarily
-- encrypted with the old (v1) derivation, if at all — v2 did not exist yet —
-- so DEFAULT 1 is not a guess, it's what those rows are actually true of.
ALTER TABLE database_backups
    ADD COLUMN IF NOT EXISTS encryption_key_version SMALLINT NOT NULL DEFAULT 1;
