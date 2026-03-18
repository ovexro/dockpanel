-- Monitoring Batch 1: TCP/port monitoring, keyword checks
ALTER TABLE monitors ADD COLUMN monitor_type VARCHAR(20) NOT NULL DEFAULT 'http';
ALTER TABLE monitors ADD COLUMN port INTEGER;
ALTER TABLE monitors ADD COLUMN keyword VARCHAR(500);
ALTER TABLE monitors ADD COLUMN keyword_must_contain BOOLEAN NOT NULL DEFAULT TRUE;
