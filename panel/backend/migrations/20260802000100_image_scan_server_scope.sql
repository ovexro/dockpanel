-- An image scan result belongs to the host that ran the scanner.
--
-- `image_scan_findings` has no server column at all and `image_sbom` is keyed
-- on the bare image string, so two hosts running the same image — which is the
-- normal case, they deploy from the same 151-template catalogue — overwrite and
-- evict each other's results. The consequences are not cosmetic:
--
--   * the Apps page badges a member's containers with whichever host scanned
--     that image most recently;
--   * the deploy gate reads the same bare-image row, so a clean scan on the
--     panel can wave a vulnerable image onto a member, and a member's dirty
--     scan can block a deploy on the panel;
--   * the 30-row history trim is per image rather than per (host, image), so a
--     busy host evicts a quiet host's only result;
--   * the sweeper's freshness check is per image, so one host's fresh scan
--     suppresses every other host's rescan indefinitely.
--
-- ⚠ `routes/sboms.rs` writes `ON CONFLICT (image)`. Once the primary key is
-- composite that arbiter matches no unique index and Postgres raises 42P10 at
-- EXECUTION time — every SBOM generation would 500. The writer is updated in
-- the same commit; this note is here because the two changes are one change.

-- ── image_scan_findings ────────────────────────────────────────────────────
ALTER TABLE image_scan_findings
    ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

UPDATE image_scan_findings
   SET server_id = (SELECT id FROM servers WHERE is_local = true LIMIT 1)
 WHERE server_id IS NULL;

DELETE FROM image_scan_findings WHERE server_id IS NULL;

ALTER TABLE image_scan_findings ALTER COLUMN server_id SET NOT NULL;

-- Every read is now (server_id, image) newest-first — the freshness check, the
-- per-app lookup, the deploy gate and the history trim alike.
DROP INDEX IF EXISTS idx_image_scan_findings_image;
CREATE INDEX IF NOT EXISTS idx_image_scan_findings_server_image
    ON image_scan_findings (server_id, image, scanned_at DESC);

-- ── image_sbom ─────────────────────────────────────────────────────────────
ALTER TABLE image_sbom
    ADD COLUMN IF NOT EXISTS server_id UUID REFERENCES servers(id) ON DELETE CASCADE;

UPDATE image_sbom
   SET server_id = (SELECT id FROM servers WHERE is_local = true LIMIT 1)
 WHERE server_id IS NULL;

DELETE FROM image_sbom WHERE server_id IS NULL;

ALTER TABLE image_sbom ALTER COLUMN server_id SET NOT NULL;

-- The bare-image primary key IS the collision. Replace it rather than add a
-- second unique index, so there is no longer any arbiter a writer could name
-- that would silently merge two hosts' documents.
ALTER TABLE image_sbom DROP CONSTRAINT IF EXISTS image_sbom_pkey;
ALTER TABLE image_sbom ADD PRIMARY KEY (server_id, image);
