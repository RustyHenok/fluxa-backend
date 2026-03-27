use chrono::Utc;
use serde_json::json;
use sqlx::{Encode, Postgres, QueryBuilder, Type};
use uuid::Uuid;

use super::Database;
use crate::domain::{
    CreateTaskInput, DashboardSummary, PaginatedTaskAudit, PaginatedTasks, TaskAuditRecord,
    TaskFilters, TaskPriority, TaskRecord, TaskStatus, UpdateTaskInput,
};
use crate::error::{AppError, AppResult};
use crate::pagination::{AuditCursor, Cursor};

impl Database {
    pub async fn dashboard_summary(&self, tenant_id: Uuid) -> AppResult<DashboardSummary> {
        sqlx::query_as::<_, DashboardSummary>(
            r#"
            SELECT
                COUNT(*) FILTER (WHERE status = 'open')::BIGINT AS open_task_count,
                COUNT(*) FILTER (WHERE status = 'in_progress')::BIGINT AS in_progress_task_count,
                COUNT(*) FILTER (WHERE status = 'done')::BIGINT AS done_task_count,
                COUNT(*) FILTER (
                    WHERE due_at IS NOT NULL
                      AND due_at <= now()
                      AND status NOT IN ('done', 'archived')
                )::BIGINT AS overdue_task_count,
                (
                    SELECT COUNT(*)::BIGINT
                    FROM task_audit_log
                    WHERE tenant_id = $1
                      AND created_at >= now() - interval '7 days'
                ) AS recent_activity_count
            FROM tasks
            WHERE tenant_id = $1
            "#,
        )
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn create_task(
        &self,
        tenant_id: Uuid,
        actor_id: Uuid,
        input: CreateTaskInput,
    ) -> AppResult<TaskRecord> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();
        let task = sqlx::query_as::<_, TaskRecord>(
            r#"
            INSERT INTO tasks (
                id,
                tenant_id,
                title,
                description,
                status,
                priority,
                assignee_id,
                due_at,
                created_by,
                updated_by,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9, $10, $10)
            RETURNING id, tenant_id, title, description, status, priority, assignee_id, due_at,
                      created_by, updated_by, created_at, updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(input.title.trim())
        .bind(input.description.clone())
        .bind(input.status.unwrap_or(TaskStatus::Open).as_str())
        .bind(input.priority.unwrap_or(TaskPriority::Medium).as_str())
        .bind(input.assignee_id)
        .bind(input.due_at)
        .bind(actor_id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_created', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task.id)
        .bind(tenant_id)
        .bind(actor_id)
        .bind(json!({
            "title": task.title,
            "status": task.status,
            "priority": task.priority,
            "assignee_id": task.assignee_id,
            "due_at": task.due_at,
        }))
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(task)
    }

    pub async fn get_task(&self, tenant_id: Uuid, task_id: Uuid) -> AppResult<Option<TaskRecord>> {
        sqlx::query_as::<_, TaskRecord>(
            r#"
            SELECT id, tenant_id, title, description, status, priority, assignee_id, due_at,
                   created_by, updated_by, created_at, updated_at
            FROM tasks
            WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_tasks(
        &self,
        tenant_id: Uuid,
        filters: &TaskFilters,
        cursor: Option<&Cursor>,
        limit: usize,
    ) -> AppResult<PaginatedTasks> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, tenant_id, title, description, status, priority, assignee_id, due_at,
                   created_by, updated_by, created_at, updated_at
            FROM tasks
            WHERE tenant_id = "#,
        );
        builder.push_bind(tenant_id);
        apply_task_filters(&mut builder, filters, cursor);
        builder.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
        builder.push_bind((limit + 1) as i64);

        let mut tasks = builder
            .build_query_as::<TaskRecord>()
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if tasks.len() > limit {
            tasks.truncate(limit);
            tasks.last().map(|task| Cursor {
                updated_at: task.updated_at,
                id: task.id,
            })
        } else {
            None
        };

        Ok(PaginatedTasks { tasks, next_cursor })
    }

    pub async fn list_task_audit(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        cursor: Option<&AuditCursor>,
        limit: usize,
    ) -> AppResult<PaginatedTaskAudit> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, task_id, tenant_id, actor_user_id, event_type, payload, created_at
            FROM task_audit_log
            WHERE tenant_id = "#,
        );
        builder.push_bind(tenant_id);
        builder.push(" AND task_id = ");
        builder.push_bind(task_id);

        if let Some(cursor) = cursor {
            builder.push(" AND (created_at < ");
            builder.push_bind(cursor.created_at);
            builder.push(" OR (created_at = ");
            builder.push_bind(cursor.created_at);
            builder.push(" AND id < ");
            builder.push_bind(cursor.id);
            builder.push("))");
        }

        builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind((limit + 1) as i64);

        let mut entries = builder
            .build_query_as::<TaskAuditRecord>()
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if entries.len() > limit {
            entries.truncate(limit);
            entries.last().map(|entry| AuditCursor {
                created_at: entry.created_at,
                id: entry.id,
            })
        } else {
            None
        };

        Ok(PaginatedTaskAudit {
            entries,
            next_cursor,
        })
    }

    pub async fn update_task(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        actor_id: Uuid,
        input: UpdateTaskInput,
    ) -> AppResult<TaskRecord> {
        let mut builder = QueryBuilder::<Postgres>::new("UPDATE tasks SET ");
        let mut needs_separator = false;
        let mut changed = 0usize;

        if let Some(title) = input.title.as_ref() {
            push_update_assignment(&mut builder, &mut needs_separator, "title", title.trim());
            changed += 1;
        }

        if let Some(description) = input.description.clone() {
            push_update_assignment(
                &mut builder,
                &mut needs_separator,
                "description",
                description,
            );
            changed += 1;
        }

        if let Some(status) = input.status.as_ref() {
            push_update_assignment(
                &mut builder,
                &mut needs_separator,
                "status",
                status.as_str(),
            );
            changed += 1;
        }

        if let Some(priority) = input.priority.as_ref() {
            push_update_assignment(
                &mut builder,
                &mut needs_separator,
                "priority",
                priority.as_str(),
            );
            changed += 1;
        }

        if let Some(assignee_id) = input.assignee_id {
            push_update_assignment(
                &mut builder,
                &mut needs_separator,
                "assignee_id",
                assignee_id,
            );
            changed += 1;
        }

        if let Some(due_at) = input.due_at {
            push_update_assignment(&mut builder, &mut needs_separator, "due_at", due_at);
            changed += 1;
        }

        if changed == 0 {
            return Err(AppError::Validation(
                "at least one task field must be provided".into(),
            ));
        }

        let now = Utc::now();
        push_update_assignment(&mut builder, &mut needs_separator, "updated_by", actor_id);
        push_update_assignment(&mut builder, &mut needs_separator, "updated_at", now);
        builder.push(" WHERE tenant_id = ");
        builder.push_bind(tenant_id);
        builder.push(" AND id = ");
        builder.push_bind(task_id);
        builder.push(
            " RETURNING id, tenant_id, title, description, status, priority, assignee_id, due_at,
                      created_by, updated_by, created_at, updated_at",
        );

        let mut tx = self.pool.begin().await?;
        let task = builder
            .build_query_as::<TaskRecord>()
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::NotFound("task not found".into()))?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_updated', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task.id)
        .bind(task.tenant_id)
        .bind(actor_id)
        .bind(json!({
            "title": input.title,
            "description": input.description,
            "status": input.status,
            "priority": input.priority,
            "assignee_id": input.assignee_id,
            "due_at": input.due_at,
        }))
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(task)
    }

    pub async fn delete_task(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        actor_id: Uuid,
    ) -> AppResult<TaskRecord> {
        let mut tx = self.pool.begin().await?;
        let task = sqlx::query_as::<_, TaskRecord>(
            r#"
            DELETE FROM tasks
            WHERE tenant_id = $1 AND id = $2
            RETURNING id, tenant_id, title, description, status, priority, assignee_id, due_at,
                      created_by, updated_by, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::NotFound("task not found".into()))?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_deleted', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task.id)
        .bind(task.tenant_id)
        .bind(actor_id)
        .bind(json!({
            "title": task.title,
            "status": task.status,
            "priority": task.priority,
        }))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(task)
    }

    pub async fn export_tasks(
        &self,
        tenant_id: Uuid,
        filters: &TaskFilters,
        limit: usize,
    ) -> AppResult<Vec<TaskRecord>> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, tenant_id, title, description, status, priority, assignee_id, due_at,
                   created_by, updated_by, created_at, updated_at
            FROM tasks
            WHERE tenant_id = "#,
        );
        builder.push_bind(tenant_id);
        apply_task_filters(&mut builder, filters, None);
        builder.push(" ORDER BY updated_at DESC, id DESC LIMIT ");
        builder.push_bind(limit as i64);

        builder
            .build_query_as::<TaskRecord>()
            .fetch_all(&self.pool)
            .await
            .map_err(AppError::from)
    }

    pub async fn record_due_reminders(&self, tenant_id: Option<Uuid>) -> AppResult<usize> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, tenant_id, title, description, status, priority, assignee_id, due_at,
                   created_by, updated_by, created_at, updated_at
            FROM tasks
            WHERE due_at IS NOT NULL
              AND due_at <= now()
              AND status NOT IN ('done', 'archived')
            "#,
        );

        if let Some(tenant_id) = tenant_id {
            builder.push(" AND tenant_id = ");
            builder.push_bind(tenant_id);
        }

        builder.push(" ORDER BY due_at ASC LIMIT 200");
        let tasks = builder
            .build_query_as::<TaskRecord>()
            .fetch_all(&self.pool)
            .await?;

        let mut tx = self.pool.begin().await?;
        for task in &tasks {
            sqlx::query(
                r#"
                INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
                VALUES ($1, $2, $3, $4, 'due_reminder_scheduled', $5, $6)
                "#,
            )
            .bind(Uuid::new_v4())
            .bind(task.id)
            .bind(task.tenant_id)
            .bind(task.updated_by)
            .bind(json!({
                "task_id": task.id,
                "due_at": task.due_at,
                "status": task.status,
            }))
            .bind(Utc::now())
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(tasks.len())
    }
}

fn push_update_assignment<'args, T>(
    builder: &mut QueryBuilder<'args, Postgres>,
    needs_separator: &mut bool,
    column: &str,
    value: T,
) where
    T: 'args + Encode<'args, Postgres> + Type<Postgres>,
{
    if *needs_separator {
        builder.push(", ");
    }

    builder.push(column);
    builder.push(" = ");
    builder.push_bind(value);
    *needs_separator = true;
}

fn apply_task_filters<'a>(
    builder: &mut QueryBuilder<'a, Postgres>,
    filters: &'a TaskFilters,
    cursor: Option<&'a Cursor>,
) {
    if let Some(status) = filters.status.as_ref() {
        builder.push(" AND status = ");
        builder.push_bind(status.as_str());
    }

    if let Some(priority) = filters.priority.as_ref() {
        builder.push(" AND priority = ");
        builder.push_bind(priority.as_str());
    }

    if let Some(assignee_id) = filters.assignee_id {
        builder.push(" AND assignee_id = ");
        builder.push_bind(assignee_id);
    }

    if let Some(due_before) = filters.due_before {
        builder.push(" AND due_at <= ");
        builder.push_bind(due_before);
    }

    if let Some(due_after) = filters.due_after {
        builder.push(" AND due_at >= ");
        builder.push_bind(due_after);
    }

    if let Some(updated_after) = filters.updated_after {
        builder.push(" AND updated_at >= ");
        builder.push_bind(updated_after);
    }

    if let Some(query) = filters.q.as_ref() {
        let like = format!("%{}%", query.trim());
        builder.push(" AND (title ILIKE ");
        builder.push_bind(like.clone());
        builder.push(" OR COALESCE(description, '') ILIKE ");
        builder.push_bind(like);
        builder.push(")");
    }

    if let Some(cursor) = cursor {
        builder.push(" AND (updated_at < ");
        builder.push_bind(cursor.updated_at);
        builder.push(" OR (updated_at = ");
        builder.push_bind(cursor.updated_at);
        builder.push(" AND id < ");
        builder.push_bind(cursor.id);
        builder.push("))");
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use sqlx::{Postgres, QueryBuilder};
    use uuid::Uuid;

    use super::push_update_assignment;

    #[test]
    fn update_assignment_builder_keeps_set_clause_valid() {
        let mut builder = QueryBuilder::<Postgres>::new("UPDATE tasks SET ");
        let mut needs_separator = false;

        push_update_assignment(&mut builder, &mut needs_separator, "status", "in_progress");
        push_update_assignment(&mut builder, &mut needs_separator, "priority", "urgent");
        push_update_assignment(
            &mut builder,
            &mut needs_separator,
            "updated_by",
            Uuid::nil(),
        );
        push_update_assignment(&mut builder, &mut needs_separator, "updated_at", Utc::now());

        assert_eq!(
            builder.sql(),
            "UPDATE tasks SET status = $1, priority = $2, updated_by = $3, updated_at = $4"
        );
    }
}
