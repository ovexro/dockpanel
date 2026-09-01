-- `staging::create` checks "does a staging site already exist for this parent"
-- with a plain SELECT-then-INSERT, no transaction or lock — and until now
-- nothing backed that invariant at the schema level: `parent_site_id` only ever
-- had a plain index (`idx_sites_parent`, `20260312700000_staging_sites.sql`),
-- not a unique one. Two concurrent `POST /sites/{id}/staging` calls for the
-- same site can both read "none exists" before either INSERT commits, landing
-- two staging rows — a race that is real (no lock anywhere in staging.rs,
-- domain_claim.rs, or sites.rs prevents it) even though the duplicate turns out
-- to be visible and deletable through the ordinary site list/delete endpoints
-- (neither of those filters on parent_site_id), not an orphan.
--
-- Partial, matching `idx_sites_parent`'s own predicate: a NULL parent_site_id
-- means "not a staging site", and any number of ordinary sites are in that
-- state.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_parent_unique
    ON sites(parent_site_id) WHERE parent_site_id IS NOT NULL;
