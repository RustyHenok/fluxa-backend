CREATE TABLE IF NOT EXISTS task_comments (
    id UUID PRIMARY KEY,
    task_id UUID NOT NULL REFERENCES tasks (id) ON DELETE CASCADE,
    tenant_id UUID NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    author_id UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    body TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_task_comments_task
    ON task_comments (task_id, created_at DESC, id DESC);
