-- Allow the 'client' role value on users.role.
--
-- A `client` is a principal who holds a set of EXISTING sites and manages them
-- (mail, PHP version, containers, settings) but cannot bring a new domain into
-- service. Requested on GitHub #51 by an operator migrating from ISPConfig,
-- where this is the ordinary shape: a client owns its domains, and the admin
-- sees them by being admin rather than by co-owning them.
--
-- Ownership stays exactly where it was — `sites.user_id`, one owner, no ACL and
-- no second axis. That is the point rather than a shortcut: every one of the 108
-- ownership-scoped reads in the backend already says `WHERE user_id = $1`, so a
-- client who OWNS the row passes all of them natively. Widening those reads to
-- "owner OR delegate" instead would have had to keep 57 INSERTs that stamp
-- `claims.sub` in step, and two name-keyed cleanups whose own source comments
-- record that they already shipped a cross-account delete once.
--
-- Deny-by-default is what makes the new value safe to add: 'client' is not
-- 'admin' and not 'reseller', so it fails `require_admin` at all 155 of its call
-- sites, plus the AdminUser and ResellerUser extractors, with no edit to any of
-- them. The only thing that had to be written by hand is the refusal to claim a
-- NEW domain, and that lives at the single choke point every domain-introducing
-- path already funnels through (services::domain_claim::ensure_claimable).
--
-- Idempotent: DROP IF EXISTS then re-ADD. No existing row violates the widened
-- set, because no row can currently hold a value outside the previous four.
ALTER TABLE users DROP CONSTRAINT IF EXISTS chk_users_role;
ALTER TABLE users ADD CONSTRAINT chk_users_role
    CHECK (role IN ('admin', 'reseller', 'user', 'suspended', 'client'));
