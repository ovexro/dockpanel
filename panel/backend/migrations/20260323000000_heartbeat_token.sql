-- Add heartbeat_token column to monitors for secure heartbeat endpoint validation
ALTER TABLE monitors ADD COLUMN heartbeat_token VARCHAR(255);

-- Backfill existing heartbeat monitors with a random token
UPDATE monitors SET heartbeat_token = gen_random_uuid()::text WHERE monitor_type = 'heartbeat' AND heartbeat_token IS NULL;
