-- How a certificate was issued — so a renewal can re-order the same thing.
--
-- `sites` recorded nothing about the ACME challenge that produced a
-- certificate, so every renewal door aimed an HTTP-01, single-name order at
-- `site.domain` regardless. For a DNS-01 certificate that is wrong in two
-- different directions, and both of them are silent:
--
--   SHAPE A — the site IS the Cloudflare zone apex. The certificate directory
--     is named after the site, so `foreign_cert_issuer` finds the wildcard,
--     sees a Let's Encrypt issuer, and returns "not foreign" — permission to
--     proceed. `provision_cert` then overwrites the shared `fullchain.pem`
--     IN PLACE with a single-name certificate, and every sibling vhost still
--     pointing at that directory begins serving a certificate that does not
--     cover it. Reachable on a stock install: `auto_fix_safe_findings` runs on
--     every security scan with no opt-in at all.
--
--   SHAPE B — a subdomain site under a zone wildcard. There is no certificate
--     at `/etc/dockpanel/ssl/{site.domain}/`, so the guard exits on
--     `has_cert:false` before it ever reads an issuer. The renewal issues an
--     orphan single-name certificate into the site's own directory, the panel
--     then re-renders the vhost from `ssl_cert_path` and points nginx BACK at
--     the un-renewed wildcard, and stamps the NEW certificate's expiry onto the
--     row. The 45-day window never reopens, and the site goes dark behind a
--     panel that already reported "SSL certificate was automatically renewed".
--
-- The product had already written this down: CHANGELOG.md's v2.141.0 entry says
-- "renewal of a DNS-01 certificate does not run over DNS-01 at all — every
-- renewal path uses HTTP-01 ... Nothing records which challenge issued a
-- certificate." These four columns are that record.
--
-- ── Why a column, when helpers.rs says a column is not needed ────────────────
--
-- `foreign_cert_issuer`'s rationale says it tells "ours to renew" from "someone
-- else's to protect" WITHOUT A PROVENANCE COLUMN, A MIGRATION, OR A BACKFILL
-- THAT COULD ONLY GUESS. That is true, and it is a different question. It asks
-- WHOSE certificate this is. The defect above turns on WHAT IT COVERS and
-- whether we can re-order the same thing — which the issuer cannot answer for a
-- certificate we issued ourselves, and which it cannot even reach on Shape B.
--
-- ── Why nullable, and why NO derived backfill ───────────────────────────────
--
-- `ssl_wildcard` is BOOLEAN and nullable on purpose: the truth is three-state
-- (wildcard / not wildcard / not recorded). `NOT NULL DEFAULT FALSE` would
-- write a positive assertion of "not a wildcard" over every pre-existing row,
-- including the Shape A apex wildcards this migration cannot identify — and the
-- UI would then render "Wildcard: No" from a default, indistinguishable from a
-- recorded fact. The precedent is `20260416100000_acme_ari_profile.sql`, which
-- added `ssl_profile TEXT` with "NULL for pre-existing certs" and no backfill.
--
-- ⛔ An earlier draft of this change derived provenance from `ssl_cert_path`:
-- "a row whose certificate directory is not the site's own domain is serving a
-- shared zone certificate". That is NOT a fact. `rename_domain` writes only
-- `UPDATE sites SET domain = $1, updated_at = NOW()` while the agent MOVES the
-- certificate directory to the new name, so every renamed HTTP-01 site carries
-- a path naming a directory it no longer uses. In SQL that row is byte-identical
-- to a genuine wildcard child. The backfill would have branded healthy sites as
-- wildcards and refused to renew them for ever — manufacturing the exact
-- outage this migration exists to prevent, on a population that never had a
-- wildcard at all.
--
-- ── The backfill that IS sound: the panel's own activity log ────────────────
--
-- `routes/ssl.rs` has written `site.ssl.dns01` / `site.ssl.wildcard` with
-- `target_name = site.domain` since the DNS-01 door's first commit, and the
-- default activity retention is 365 days. Presence is a POSITIVE HISTORICAL
-- FACT with exactly the epistemics this needs: presence proves DNS-01, absence
-- proves nothing. It also reaches the non-wildcard DNS-01 population — the
-- likeliest and worst class, whose certificate is byte-identical to an HTTP-01
-- one and whose operator chose DNS-01 precisely because port 80 cannot work.
--
-- Three guards make it safe:
--   1. It matches on `target_name = sites.domain`, so a RENAMED site simply
--      does not match (the log keeps the old name). A false negative degrades
--      to today's behaviour, which is the safe direction.
--   2. The newest ssl-lifecycle action for that domain must BE the dns01 /
--      wildcard one, so a certificate later revoked or replaced by an upload
--      is not mislabelled.
--   3. `sites.domain` is unique only per SERVER, and `activity_logs` carries no
--      server, so a domain present on two servers is left alone entirely.
--
-- Only AFTER activity_logs has proven a row is DNS-01 is `ssl_cert_path`
-- consulted — for the SUBJECT alone. The rename hazard is gone by construction,
-- because a renamed row never reaches this clause.

ALTER TABLE sites
    ADD COLUMN IF NOT EXISTS ssl_challenge TEXT,
    ADD COLUMN IF NOT EXISTS ssl_cert_subject TEXT,
    ADD COLUMN IF NOT EXISTS ssl_wildcard BOOLEAN,
    ADD COLUMN IF NOT EXISTS ssl_dns_zone_id UUID REFERENCES dns_zones(id) ON DELETE SET NULL;

-- The zone that issued it, so a renewal uses the credential that issued it and
-- no text subject can ever select a different tenant's Cloudflare token.
CREATE INDEX IF NOT EXISTS idx_sites_ssl_dns_zone ON sites (ssl_dns_zone_id)
    WHERE ssl_dns_zone_id IS NOT NULL;

-- Provenance recovered from the activity log. See the three guards above.
WITH ssl_events AS (
    SELECT DISTINCT ON (target_name)
           target_name,
           action
      FROM activity_logs
     WHERE target_type = 'site'
       AND action IN ('site.ssl.dns01', 'site.ssl.wildcard', 'ssl.revoke', 'ssl.upload')
     ORDER BY target_name, created_at DESC
),
unambiguous AS (
    SELECT domain FROM sites GROUP BY domain HAVING count(*) = 1
)
UPDATE sites s
   SET ssl_challenge    = 'dns-01',
       ssl_wildcard     = (e.action = 'site.ssl.wildcard'),
       ssl_cert_subject = CASE
           WHEN s.ssl_cert_path LIKE '/etc/dockpanel/ssl/%/fullchain.pem'
               THEN split_part(s.ssl_cert_path, '/', 5)
           ELSE s.domain
       END
  FROM ssl_events e, unambiguous u
 WHERE e.target_name = s.domain
   AND u.domain = s.domain
   AND e.action IN ('site.ssl.dns01', 'site.ssl.wildcard')
   AND s.ssl_enabled = TRUE
   AND s.ssl_challenge IS NULL;

-- The issuing zone, where exactly one zone answers to the recovered subject.
WITH one_zone AS (
    SELECT domain, (array_agg(id))[1] AS id
      FROM dns_zones
     WHERE provider = 'cloudflare'
     GROUP BY domain
    HAVING count(*) = 1
)
UPDATE sites s
   SET ssl_dns_zone_id = z.id
  FROM one_zone z
 WHERE s.ssl_challenge = 'dns-01'
   AND s.ssl_dns_zone_id IS NULL
   AND z.domain = s.ssl_cert_subject;
