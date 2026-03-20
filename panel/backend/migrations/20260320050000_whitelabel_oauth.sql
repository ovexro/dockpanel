-- White-Label Branding: per-reseller customization
ALTER TABLE reseller_profiles ADD COLUMN IF NOT EXISTS logo_url TEXT;
ALTER TABLE reseller_profiles ADD COLUMN IF NOT EXISTS accent_color VARCHAR(20);
ALTER TABLE reseller_profiles ADD COLUMN IF NOT EXISTS hide_branding BOOLEAN NOT NULL DEFAULT FALSE;

-- OAuth / SSO Login
ALTER TABLE users ADD COLUMN IF NOT EXISTS oauth_provider VARCHAR(20);
ALTER TABLE users ADD COLUMN IF NOT EXISTS oauth_id VARCHAR(255);
CREATE INDEX IF NOT EXISTS idx_users_oauth ON users(oauth_provider, oauth_id)
    WHERE oauth_provider IS NOT NULL;
