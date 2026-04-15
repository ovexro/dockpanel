-- Per-image vulnerability scanning.
-- Distinct from the general security_scans / security_findings tables: those
-- aggregate across the whole server; this table tracks the latest scan result
-- for each individual Docker image so the Apps UI can badge per-app.

CREATE TABLE image_scan_findings (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    image VARCHAR(512) NOT NULL,
    scanner VARCHAR(32) NOT NULL DEFAULT 'grype',
    critical_count INTEGER NOT NULL DEFAULT 0,
    high_count INTEGER NOT NULL DEFAULT 0,
    medium_count INTEGER NOT NULL DEFAULT 0,
    low_count INTEGER NOT NULL DEFAULT 0,
    unknown_count INTEGER NOT NULL DEFAULT 0,
    vulnerabilities JSONB NOT NULL DEFAULT '[]',
    scanned_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Per-image lookup of latest scan
CREATE INDEX idx_image_scan_findings_image ON image_scan_findings (image, scanned_at DESC);
-- Background sweeper uses this to find stale rows
CREATE INDEX idx_image_scan_findings_scanned_at ON image_scan_findings (scanned_at);

-- Settings: defaults err on the side of "off" so this never silently changes
-- behaviour for existing installs after upgrade. Admin opts in via Settings UI.
INSERT INTO settings (key, value) VALUES
    ('image_scan_enabled', 'false'),
    ('image_scan_on_deploy', 'false'),
    ('image_scan_deploy_gate', 'none'),         -- none | critical | high | medium
    ('image_scan_interval_hours', '24')
ON CONFLICT (key) DO NOTHING;
