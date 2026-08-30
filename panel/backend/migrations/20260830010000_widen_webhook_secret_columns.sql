-- Encrypted webhook/verify secrets are longer than their plaintext (AES-GCM
-- 12-byte nonce + 16-byte tag, then base64 expansion) — these VARCHAR
-- columns were sized for the plaintext only, and would reject the
-- ciphertext once these secrets are encrypted at rest.
--   deploy_configs.webhook_secret: 32-char plaintext -> 80-char ciphertext, was VARCHAR(64)
--   git_deploys.webhook_secret:    64-char plaintext -> 124-char ciphertext, was VARCHAR(64)
--   webhook_endpoints.verify_secret: up to 200-char plaintext -> up to 304-char ciphertext, was VARCHAR(200)
ALTER TABLE deploy_configs ALTER COLUMN webhook_secret TYPE TEXT;
ALTER TABLE git_deploys ALTER COLUMN webhook_secret TYPE TEXT;
ALTER TABLE webhook_endpoints ALTER COLUMN verify_secret TYPE TEXT;
