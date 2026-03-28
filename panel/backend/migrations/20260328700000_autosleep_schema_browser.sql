-- Container auto-sleep configuration
CREATE TABLE container_sleep_config (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    container_id VARCHAR(255) NOT NULL UNIQUE,
    container_name VARCHAR(255) NOT NULL,
    domain VARCHAR(255),
    auto_sleep_enabled BOOLEAN NOT NULL DEFAULT false,
    sleep_after_minutes INT NOT NULL DEFAULT 30,
    is_sleeping BOOLEAN NOT NULL DEFAULT false,
    last_activity_at TIMESTAMPTZ,
    last_slept_at TIMESTAMPTZ,
    last_woken_at TIMESTAMPTZ,
    total_sleeps INT NOT NULL DEFAULT 0,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_container_sleep_config_container_id ON container_sleep_config(container_id);
CREATE INDEX idx_container_sleep_config_auto_sleep ON container_sleep_config(auto_sleep_enabled) WHERE auto_sleep_enabled = true;
