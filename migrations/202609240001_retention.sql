-- Allow the retention sweep job type alongside the existing job types.
ALTER TABLE background_jobs DROP CONSTRAINT IF EXISTS background_jobs_job_type_check;
ALTER TABLE background_jobs
    ADD CONSTRAINT background_jobs_job_type_check
    CHECK (job_type IN ('task_export', 'due_reminder_sweep', 'retention_sweep'));

-- Indexes supporting retention purges and per-user refresh token lookups.
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_user ON refresh_tokens (user_id);
CREATE INDEX IF NOT EXISTS idx_refresh_tokens_expires ON refresh_tokens (expires_at);
CREATE INDEX IF NOT EXISTS idx_background_jobs_terminal
    ON background_jobs (status, finished_at)
    WHERE status IN ('completed', 'dead_letter');
CREATE INDEX IF NOT EXISTS idx_notifications_terminal
    ON notifications (status, created_at)
    WHERE status IN ('sent', 'dead_letter');
CREATE INDEX IF NOT EXISTS idx_audit_log_created ON audit_log (created_at);
