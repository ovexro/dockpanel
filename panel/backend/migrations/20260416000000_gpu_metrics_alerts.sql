-- GPU metrics history for historical charts (Phase 2 #2)
CREATE TABLE IF NOT EXISTS gpu_metrics_history (
    id BIGSERIAL PRIMARY KEY,
    gpu_index SMALLINT NOT NULL,
    utilization_pct REAL NOT NULL,
    memory_used_mb REAL NOT NULL,
    memory_total_mb REAL NOT NULL,
    temperature_c REAL,
    power_draw_w REAL,
    server_id UUID REFERENCES servers(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_gpu_metrics_created ON gpu_metrics_history(created_at DESC);
CREATE INDEX idx_gpu_metrics_server ON gpu_metrics_history(server_id, gpu_index, created_at DESC);

-- GPU alert thresholds on alert_rules
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS gpu_util_threshold INTEGER NOT NULL DEFAULT 95;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS gpu_util_duration INTEGER NOT NULL DEFAULT 5;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS gpu_temp_threshold INTEGER NOT NULL DEFAULT 85;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS gpu_vram_threshold INTEGER NOT NULL DEFAULT 95;
ALTER TABLE alert_rules ADD COLUMN IF NOT EXISTS alert_gpu BOOLEAN NOT NULL DEFAULT TRUE;

-- Data integrity constraints
ALTER TABLE alert_rules ADD CONSTRAINT chk_gpu_util_threshold
    CHECK (gpu_util_threshold BETWEEN 1 AND 100);
ALTER TABLE alert_rules ADD CONSTRAINT chk_gpu_temp_threshold
    CHECK (gpu_temp_threshold BETWEEN 1 AND 120);
ALTER TABLE alert_rules ADD CONSTRAINT chk_gpu_vram_threshold
    CHECK (gpu_vram_threshold BETWEEN 1 AND 100);
