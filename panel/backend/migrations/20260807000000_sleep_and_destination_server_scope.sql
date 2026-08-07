-- Two tables the multi-server migration left without a usable host.
--
-- `20260319000000_multi_server.sql` scoped sites, docker_stacks, git_deploys,
-- dns_zones, mail_domains, monitors, backup_destinations and activity_logs. Two
-- gaps survived it, and both made a background loop act on the panel's own box
-- for a resource living somewhere else:
--
--   1. `container_sleep_config` was never given a `server_id` at all. It was
--      created two migrations later (20260328700000_autosleep_schema_browser)
--      and simply missed the pattern. `auto_sleep_idle_containers` therefore had
--      no row to resolve a host from and took the panel's local agent: it asked
--      the LOCAL nginx how recently a member's domain was served (always
--      "unknown", so real traffic never counted), listed the LOCAL host's
--      containers to decide whether the app was running, and posted the stop to
--      the LOCAL agent. Auto-sleep was silently inoperative for every fleet
--      member, and a container id that happened to collide would have been
--      stopped on the wrong machine.
--
--   2. `backup_destinations` DID get the column (multi_server.sql:26, nullable
--      because a destination can legitimately be shared) but was left out of the
--      backfill block at :65-72. Every row predating this migration is therefore
--      NULL, which is not "shared" — it is "never answered". A NULL there is
--      indistinguishable from a deliberate fleet-wide destination, so the column
--      could not be read by anything without guessing.
--
-- Both backfills follow the original's rule: data that predates the fleet ran on
-- the local box, because there was nowhere else for it to run.

-- ── 1. container_sleep_config ───────────────────────────────────────────────
ALTER TABLE container_sleep_config
    ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_container_sleep_config_server_id
    ON container_sleep_config(server_id);

DO $$
DECLARE
    local_sid UUID;
BEGIN
    SELECT id INTO local_sid FROM servers WHERE is_local = TRUE LIMIT 1;

    -- Prefer the truth we actually hold: where a sleep row names a domain that
    -- a site serves, that site's row already answers which machine it runs on.
    -- This is the only arm that can recover a MEMBER's rows correctly, so it
    -- runs before the local default rather than after it.
    UPDATE container_sleep_config csc
       SET server_id = s.server_id
      FROM sites s
     WHERE csc.server_id IS NULL
       AND csc.domain IS NOT NULL
       AND csc.domain <> ''
       AND lower(csc.domain) = lower(s.domain);

    -- Everything else predates the fleet or has no domain to match on. Those
    -- containers were created through the local agent by construction.
    IF local_sid IS NOT NULL THEN
        UPDATE container_sleep_config SET server_id = local_sid WHERE server_id IS NULL;
    END IF;
END $$;

-- ── 2. backup_destinations — the backfill the multi-server migration skipped ──
DO $$
DECLARE
    local_sid UUID;
BEGIN
    SELECT id INTO local_sid FROM servers WHERE is_local = TRUE LIMIT 1;

    IF local_sid IS NOT NULL THEN
        UPDATE backup_destinations SET server_id = local_sid WHERE server_id IS NULL;
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_backup_destinations_server_id
    ON backup_destinations(server_id);

-- ── 3. mail_domains — backfilled once in 2026-03, then re-broken immediately ──
--
-- The worst of the three, because this one was NOT a missed table. multi_server
-- .sql:20 added the column and :70 backfilled it — and then `mail::create_domain`
-- went on inserting `(domain, dkim_private_key, dkim_public_key)` without ever
-- naming `server_id`. The backfill only ever touched rows that existed the second
-- it ran, so EVERY mail domain created through the panel since 2026-03-19 has a
-- NULL host. (`create_domain` now binds it; this recovers the rows it already
-- wrote.)
--
-- Why this is blocking rather than cosmetic: `/mail/sync` REBUILDS Postfix and
-- Dovecot maps, it does not merge them. Once `sync_mail_config` filters by server
-- — which it must, since it was previously shipping every mailbox's password_hash
-- in the installation to one host — a domain with a NULL server is omitted from
-- every payload, and omission from a rebuild DELETES that domain's mailboxes from
-- the host actually serving it. A NULL here is not evidence of the local box; it
-- is the absence of evidence, and the two must not be conflated on a path that
-- destroys mail. The runtime guard refuses the rebuild while any NULL row exists;
-- this migration is what clears that refusal.
--
-- A second, quieter consequence: idx_mail_domains_domain_server is on
-- (domain, server_id) and cannot fire on NULLs, while the migration dropped the
-- old global mail_domains_domain_key. Duplicate mail domains have been insertable
-- ever since, and create_domain's ON CONFLICT arm has been dead code.
DO $$
DECLARE
    local_sid UUID;
    remaining INT;
BEGIN
    SELECT id INTO local_sid FROM servers WHERE is_local = TRUE LIMIT 1;

    -- Recoverable arm first: a mail domain that matches a site's domain belongs
    -- on that site's host. This is the only arm that can put a domain
    -- provisioned onto a fleet member back where it belongs instead of
    -- defaulting it to this box.
    UPDATE mail_domains md
       SET server_id = s.server_id
      FROM sites s
     WHERE md.server_id IS NULL
       AND lower(md.domain) = lower(s.domain);

    IF local_sid IS NOT NULL THEN
        UPDATE mail_domains SET server_id = local_sid WHERE server_id IS NULL;
    END IF;

    -- Make the invariant structural where we can. Guarded rather than
    -- unconditional: on an install with no local server row yet there is nothing
    -- to backfill from, and a migration that aborts is worse than a column that
    -- stays nullable with the runtime precondition still standing.
    SELECT COUNT(*) INTO remaining FROM mail_domains WHERE server_id IS NULL;
    IF remaining = 0 THEN
        ALTER TABLE mail_domains ALTER COLUMN server_id SET NOT NULL;
    ELSE
        RAISE WARNING 'mail_domains still has % row(s) with no server_id; leaving the column nullable and the runtime guard in force', remaining;
    END IF;
END $$;
