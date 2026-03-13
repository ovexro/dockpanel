-- Staging environments: link staging sites to their production parent
ALTER TABLE sites ADD COLUMN parent_site_id UUID REFERENCES sites(id) ON DELETE SET NULL;
ALTER TABLE sites ADD COLUMN synced_at TIMESTAMPTZ;

CREATE INDEX idx_sites_parent ON sites(parent_site_id) WHERE parent_site_id IS NOT NULL;
