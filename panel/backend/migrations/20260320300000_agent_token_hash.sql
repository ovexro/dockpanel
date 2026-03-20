-- Add hashed agent token column for secure storage.
-- The plaintext agent_token column is kept during transition for remote agent communication.

ALTER TABLE servers ADD COLUMN IF NOT EXISTS agent_token_hash VARCHAR(64);

-- Populate hashes from existing plaintext tokens
UPDATE servers SET agent_token_hash = encode(sha256(agent_token::bytea), 'hex')
WHERE agent_token_hash IS NULL AND agent_token IS NOT NULL AND agent_token != '';

-- Index on hash for fast lookup during agent checkin
CREATE INDEX IF NOT EXISTS idx_servers_token_hash ON servers(agent_token_hash);
