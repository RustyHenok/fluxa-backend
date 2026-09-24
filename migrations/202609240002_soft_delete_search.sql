-- Soft delete for projects: archived projects stay in the table but are
-- excluded from listings until restored.
ALTER TABLE projects ADD COLUMN IF NOT EXISTS archived_at TIMESTAMPTZ;

CREATE INDEX IF NOT EXISTS idx_projects_tenant_active
    ON projects (tenant_id, updated_at DESC)
    WHERE archived_at IS NULL;

-- Full-text search over task title and description with an ILIKE fallback
-- for short terms handled in application code.
ALTER TABLE tasks ADD COLUMN IF NOT EXISTS search_tsv tsvector
    GENERATED ALWAYS AS (
        to_tsvector('simple', coalesce(title, '') || ' ' || coalesce(description, ''))
    ) STORED;

CREATE INDEX IF NOT EXISTS idx_tasks_search_tsv
    ON tasks USING GIN (search_tsv);
