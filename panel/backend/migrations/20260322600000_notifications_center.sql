-- Notification Center: in-panel notifications with bell icon + unread badge
CREATE TABLE IF NOT EXISTS panel_notifications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    title TEXT NOT NULL,
    message TEXT NOT NULL,
    severity TEXT NOT NULL DEFAULT 'info',
    category TEXT NOT NULL DEFAULT 'system',
    link TEXT,
    read_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_panel_notifications_user ON panel_notifications(user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_panel_notifications_unread ON panel_notifications(user_id) WHERE read_at IS NULL;
