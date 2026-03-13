-- Add live metrics columns to servers for dashboard display
ALTER TABLE servers ADD COLUMN cpu_usage REAL;
ALTER TABLE servers ADD COLUMN mem_used_mb BIGINT;
ALTER TABLE servers ADD COLUMN uptime_secs BIGINT;
