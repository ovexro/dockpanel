-- Nixpacks + Preview Environments Enhancement

-- Track which build method was used (dockerfile, nixpacks, auto-detect, compose)
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS build_method VARCHAR(20) NOT NULL DEFAULT 'auto';

-- Configurable TTL for preview environments (hours, 0 = no auto-cleanup)
ALTER TABLE git_deploys ADD COLUMN IF NOT EXISTS preview_ttl_hours INT NOT NULL DEFAULT 24;

-- Track when previews were last updated (for TTL cleanup)
ALTER TABLE git_previews ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW();
