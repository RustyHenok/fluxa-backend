use chrono::Utc;
use sqlx::{Encode, Postgres, QueryBuilder, Type};
use uuid::Uuid;

use super::Database;
use crate::domain::{CreateProjectInput, ProjectRecord, ProjectSummary, UpdateProjectInput};
use crate::error::{AppError, AppResult};

impl Database {
    pub async fn create_project(
        &self,
        tenant_id: Uuid,
        actor_id: Uuid,
        input: CreateProjectInput,
    ) -> AppResult<ProjectRecord> {
        let now = Utc::now();

        sqlx::query_as::<_, ProjectRecord>(
            r#"
            INSERT INTO projects (
                id,
                tenant_id,
                name,
                description,
                created_by,
                updated_by,
                created_at,
                updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $5, $6, $6)
            RETURNING id, tenant_id, name, description, created_by, updated_by, created_at, updated_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(input.name.trim())
        .bind(input.description)
        .bind(actor_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_projects(&self, tenant_id: Uuid) -> AppResult<Vec<ProjectRecord>> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
            SELECT id, tenant_id, name, description, created_by, updated_by, created_at, updated_at
            FROM projects
            WHERE tenant_id = $1
            ORDER BY updated_at DESC, id DESC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_project(
        &self,
        tenant_id: Uuid,
        project_id: Uuid,
    ) -> AppResult<Option<ProjectRecord>> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
            SELECT id, tenant_id, name, description, created_by, updated_by, created_at, updated_at
            FROM projects
            WHERE tenant_id = $1 AND id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn project_summary(
        &self,
        tenant_id: Uuid,
        project_id: Uuid,
    ) -> AppResult<Option<ProjectSummary>> {
        sqlx::query_as::<_, ProjectSummary>(
            r#"
            SELECT
                p.id AS project_id,
                p.name AS project_name,
                COUNT(t.id) FILTER (WHERE t.status = 'open')::BIGINT AS open_task_count,
                COUNT(t.id) FILTER (WHERE t.status = 'in_progress')::BIGINT AS in_progress_task_count,
                COUNT(t.id) FILTER (WHERE t.status = 'done')::BIGINT AS done_task_count,
                COUNT(t.id) FILTER (
                    WHERE t.due_at IS NOT NULL
                      AND t.due_at <= now()
                      AND t.status NOT IN ('done', 'archived')
                )::BIGINT AS overdue_task_count,
                (
                    SELECT COUNT(*)::BIGINT
                    FROM task_audit_log audit
                    INNER JOIN tasks audited_task
                        ON audited_task.id = audit.task_id
                       AND audited_task.tenant_id = audit.tenant_id
                    WHERE audit.tenant_id = p.tenant_id
                      AND audited_task.project_id = p.id
                      AND audit.created_at >= now() - interval '7 days'
                ) AS recent_activity_count
            FROM projects p
            LEFT JOIN tasks t
                ON t.project_id = p.id
               AND t.tenant_id = p.tenant_id
            WHERE p.tenant_id = $1 AND p.id = $2
            GROUP BY p.id, p.name
            "#,
        )
        .bind(tenant_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn update_project(
        &self,
        tenant_id: Uuid,
        project_id: Uuid,
        actor_id: Uuid,
        input: UpdateProjectInput,
    ) -> AppResult<ProjectRecord> {
        let mut builder = QueryBuilder::<Postgres>::new("UPDATE projects SET ");
        let mut needs_separator = false;

        if let Some(name) = input.name.as_ref() {
            push_update_assignment(&mut builder, &mut needs_separator, "name", name.trim());
        }

        if let Some(description) = input.description {
            push_update_assignment(
                &mut builder,
                &mut needs_separator,
                "description",
                description,
            );
        }

        push_update_assignment(&mut builder, &mut needs_separator, "updated_by", actor_id);
        push_update_assignment(&mut builder, &mut needs_separator, "updated_at", Utc::now());
        builder.push(" WHERE tenant_id = ");
        builder.push_bind(tenant_id);
        builder.push(" AND id = ");
        builder.push_bind(project_id);
        builder.push(
            " RETURNING id, tenant_id, name, description, created_by, updated_by, created_at, updated_at",
        );

        builder
            .build_query_as::<ProjectRecord>()
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| AppError::NotFound("project not found".into()))
    }

    pub async fn delete_project(
        &self,
        tenant_id: Uuid,
        project_id: Uuid,
    ) -> AppResult<ProjectRecord> {
        sqlx::query_as::<_, ProjectRecord>(
            r#"
            DELETE FROM projects
            WHERE tenant_id = $1 AND id = $2
            RETURNING id, tenant_id, name, description, created_by, updated_by, created_at, updated_at
            "#,
        )
        .bind(tenant_id)
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("project not found".into()))
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
