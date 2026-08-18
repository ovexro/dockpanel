-- The port allocators are scoped to a server. Two of their uniqueness indexes
-- are not, and the mismatch is a regression this project already fixed once.
--
-- `20260319000000_multi_server.sql` un-globalised five columns under the header
-- "domain should be unique per server, not globally" (:80). One of them was
-- `git_deploys.host_port` at :91-94, which became
-- `idx_git_deploys_host_port_server ON git_deploys(host_port, server_id)` and has
-- agreed with its allocator ever since (`routes/git_deploys.rs:398-410`: ports
-- 7000-7999, `WHERE server_id = $1`).
--
-- The NEXT migration, one day later, re-introduced the global shape on two
-- neighbouring columns. Both allocators pick the first free port from a
-- server-scoped used-set, so on any install with more than one server the second
-- server's set is empty, it picks the bottom of the range, and the INSERT
-- collides with a row belonging to the first server. It is not a race: it is
-- deterministic, and it repeats on every attempt because the losing server never
-- records anything to make its own used-set grow.
--
--   1. `idx_git_previews_host_port` — UNIQUE on `git_previews(host_port)`, one
--      column, no predicate, against an allocator that scopes through the JOIN
--      (`routes/git_deploys.rs:3011-3026`, ports 8000-8999). The rejected INSERT
--      was logged at warn and discarded and the deploy task ran anyway, so the
--      container, its published port, its vhost and its Let's Encrypt
--      certificate were created with NO ROW. Every consumer is row-driven — the
--      previews list, Delete Preview, the TTL and stuck sweeps, the parent
--      delete — so nothing on the box could reap it, while Docker's own
--      `unless-stopped` policy carried it across crashes and reboots. The same
--      function already refuses at :3081 to overwrite a row for exactly this
--      reason, in a comment naming "the predecessor's container, port, vhost and
--      checkout are orphaned with nothing in the database naming them".
--
--   2. `idx_sites_proxy_port` — UNIQUE on `sites(proxy_port)`, partial on
--      NOT NULL but still global, against `routes/sites.rs:479-490`, which
--      allocates node/python ports from `generate_series(5000, 5999)` filtered
--      `WHERE ... server_id = $1`. Here the INSERT sits in a transaction whose
--      error arm maps any "duplicate key" to CONFLICT "Domain already exists",
--      so creating a node or python site on any server after the first failed
--      permanently under a message that sends the operator to look at DNS.
--
-- `sites` already carries `server_id` (multi_server.sql:8, NOT NULL).
-- `git_previews` does not: the TTL sweep reaches its host through the JOIN, and
-- that is enough for a query but a unique index cannot reach through a JOIN. So
-- the column is added here, with the same shape and the same ON DELETE CASCADE
-- the sibling tables use.

-- ── 1. git_previews — give it the host its own allocator already scopes by ───
ALTER TABLE git_previews
    ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

-- Total by construction: `git_previews.git_deploy_id` is NOT NULL with an FK to
-- `git_deploys`, and `git_deploys.server_id` is NOT NULL. This writes the same
-- value `services/preview_cleanup.rs` already resolves through the JOIN, so no
-- row changes host.
UPDATE git_previews p
   SET server_id = d.server_id
  FROM git_deploys d
 WHERE d.id = p.git_deploy_id
   AND p.server_id IS NULL;

DO $$
DECLARE
    remaining INT;
BEGIN
    SELECT COUNT(*) INTO remaining FROM git_previews WHERE server_id IS NULL;
    IF remaining = 0 THEN
        ALTER TABLE git_previews ALTER COLUMN server_id SET NOT NULL;
    ELSE
        -- Guarded rather than unconditional, per 20260807000000's own rule: a
        -- migration that aborts is worse than a column that stays nullable.
        RAISE WARNING 'git_previews still has % row(s) with no server_id; leaving the column nullable', remaining;
    END IF;
END $$;

-- Create the scoped index BEFORE dropping the global one. If the create fails,
-- the old (strictly stronger) guarantee is still in force rather than none.
-- Column order matches the sibling `idx_git_deploys_host_port_server`.
CREATE UNIQUE INDEX IF NOT EXISTS idx_git_previews_host_port_server
    ON git_previews(host_port, server_id);
DROP INDEX IF EXISTS idx_git_previews_host_port;

-- ── 2. sites.proxy_port — the column already knows its server ────────────────
-- The partial predicate is kept: a NULL proxy_port means "this runtime does not
-- proxy", and any number of sites may be in that state.
CREATE UNIQUE INDEX IF NOT EXISTS idx_sites_proxy_port_server
    ON sites(proxy_port, server_id) WHERE proxy_port IS NOT NULL;
DROP INDEX IF EXISTS idx_sites_proxy_port;
