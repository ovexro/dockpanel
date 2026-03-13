-- Auto-healing default setting (disabled by default — opt-in)
INSERT INTO settings (key, value) VALUES ('auto_heal_enabled', 'false') ON CONFLICT (key) DO NOTHING;
