-- Per-image SBOMs (SPDX 2.3 JSON).
-- One row per image; rescan overwrites. Stored as JSONB so the
-- Download SBOM endpoint serves directly without re-parsing on the agent.

CREATE TABLE image_sbom (
    image VARCHAR(512) PRIMARY KEY,
    format VARCHAR(32) NOT NULL DEFAULT 'spdx-json',
    spdx JSONB NOT NULL,
    generated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_image_sbom_generated_at ON image_sbom (generated_at);

-- Track install state of the syft binary on the agent so the UI can render
-- the same install/uninstall pattern as image scanning. Defaults off.
INSERT INTO settings (key, value) VALUES
    ('sbom_enabled', 'false')
ON CONFLICT (key) DO NOTHING;
