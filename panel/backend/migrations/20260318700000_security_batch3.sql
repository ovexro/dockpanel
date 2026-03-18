-- Security Batch 3: 2FA enforcement
INSERT INTO settings (key, value) VALUES ('enforce_2fa', 'false') ON CONFLICT DO NOTHING;
