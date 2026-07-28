-- Drop settings rows that nothing in the panel has ever read.
--
-- `20260324000000_security_enhancements.sql` seeded ten "security settings".
-- Two of them were never wired to anything: the DB-backup retention is hardcoded
-- (`auto_healer.rs` prunes with `skip(7)`) and the backup hash chain runs
-- unconditionally. Both rows therefore describe behaviour no code consults, which
-- reads to the next auditor as configuration and is not.
--
-- These two are safe to delete: neither was in the writable whitelist and neither
-- had a control, so no operator can ever have set them — the values here are the
-- seeds and nothing else.
DELETE FROM settings WHERE key IN (
    'security_db_backup_retention_days',
    'security_backup_chain_enabled'
);

-- NOT deleted, deliberately: `timezone`, `email_footer` and `events_webhook_url`.
-- Their controls are gone in v2.46.0 because nothing read them either, but unlike
-- the two above an operator COULD type into those fields, and dropping the rows
-- would throw away text they wrote. They are inert either way; if the features
-- they promised get built, the values are still there.
