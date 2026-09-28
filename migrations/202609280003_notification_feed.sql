ALTER TABLE notifications ADD COLUMN IF NOT EXISTS read_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_notifications_user_feed
    ON notifications (user_id, tenant_id, created_at DESC, id DESC);
