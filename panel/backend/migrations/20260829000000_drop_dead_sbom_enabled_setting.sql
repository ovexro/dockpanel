-- Drop `sbom_enabled`, a write-only settings row nothing in the panel reads.
--
-- `20260415100000_image_sbom.sql` seeded it as a placeholder for a syft-install
-- toggle mirroring image scanning's own settings row, but that gating was never
-- built: SBOM availability is actually driven by whether syft is installed on
-- the agent (`GET /api/sbom/settings` → `SbomSettings { installed }`), not by
-- this row. No route ever wrote to it either, so no operator can have set a
-- real value here — the seed is the only value that has ever existed.
DELETE FROM settings WHERE key = 'sbom_enabled';
