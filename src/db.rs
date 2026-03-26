use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use sqlx::{Encode, PgPool, Postgres, QueryBuilder, Type};
use uuid::Uuid;

use crate::config::SharedConfig;
use crate::domain::{
    BackgroundJobRecord, CreateTaskInput, JOB_STATUS_COMPLETED, JOB_STATUS_DEAD_LETTER,
    JOB_STATUS_QUEUED, JOB_TYPE_DUE_REMINDER_SWEEP, MembershipRecord, PaginatedTasks,
    RefreshTokenRecord, TaskFilters, TaskRecord, TenantRecord, UpdateTaskInput, UserRecord,
};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;

static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    pub async fn connect(config: &SharedConfig) -> AppResult<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(config.database_max_connections)
            .connect(&config.database_url)
            .await?;

        Ok(Self { pool })
    }

    pub async fn migrate(&self) -> AppResult<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn health_check(&self) -> AppResult<()> {
        let _: i32 = sqlx::query_scalar("SELECT 1").fetch_one(&self.pool).await?;
        Ok(())
    }

    pub async fn create_user_with_tenant(
        &self,
        email: &str,
        password_hash: &str,
        tenant_name: &str,
    ) -> AppResult<(UserRecord, MembershipRecord)> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();
        let user_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();

        let user = sqlx::query_as::<_, UserRecord>(
            r#"
            INSERT INTO users (id, email, password_hash, created_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, email, password_hash, created_at
            "#,
        )
        .bind(user_id)
        .bind(email)
        .bind(password_hash)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        let tenant = sqlx::query_as::<_, TenantRecord>(
            r#"
            INSERT INTO tenants (id, name, created_at)
            VALUES ($1, $2, $3)
            RETURNING id, name, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(tenant_name)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        let membership = sqlx::query_as::<_, MembershipRecord>(
            r#"
            INSERT INTO tenant_memberships (tenant_id, user_id, role, created_at)
            VALUES ($1, $2, 'owner', $3)
            RETURNING tenant_id,
                      $4 AS tenant_name,
                      user_id,
                      role,
                      created_at
            "#,
        )
        .bind(tenant.id)
        .bind(user.id)
        .bind(now)
        .bind(tenant.name.clone())
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok((user, membership))
    }

    pub async fn get_user_by_email(&self, email: &str) -> AppResult<Option<UserRecord>> {
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, email, password_hash, created_at
            FROM users
            WHERE email = $1
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_user_by_id(&self, user_id: Uuid) -> AppResult<UserRecord> {
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, email, password_hash, created_at
            FROM users
            WHERE id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))
    }

    pub async fn list_memberships(&self, user_id: Uuid) -> AppResult<Vec<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1
            ORDER BY tm.created_at ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_membership(
        &self,
        user_id: Uuid,
        tenant_id: Uuid,
    ) -> AppResult<Option<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1 AND tm.tenant_id = $2
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_default_membership(
        &self,
        user_id: Uuid,
    ) -> AppResult<Option<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1
            ORDER BY tm.created_at ASC
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn create_refresh_token(
        &self,
        token_id: Uuid,
        user_id: Uuid,
        tenant_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (id, user_id, tenant_id, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(token_id)
        .bind(user_id)
        .bind(tenant_id)
        .bind(expires_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_refresh_token(&self, token_id: Uuid) -> AppResult<Option<RefreshTokenRecord>> {
        sqlx::query_as::<_, RefreshTokenRecord>(
            r#"
            SELECT id, user_id, tenant_id, expires_at, revoked_at, replaced_by, created_at
            FROM refresh_tokens
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn rotate_refresh_token(
        &self,
        previous_token_id: Uuid,
        next_token_id: Uuid,
        user_id: Uuid,
        tenant_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();

        let updated = sqlx::query(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = $2, replaced_by = $3
            WHERE id = $1 AND revoked_at IS NULL AND expires_at > $2
            "#,
        )
        .bind(previous_token_id)
        .bind(now)
        .bind(next_token_id)
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() == 0 {
            return Err(AppError::Unauthorized(
                "refresh token is expired or already revoked".into(),
            ));
        }

        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (id, user_id, tenant_id, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(next_token_id)
        .bind(user_id)
        .bind(tenant_id)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn revoke_refresh_token(&self, token_id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = COALESCE(revoked_at, $2)
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
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
        .bind(input.status.unwrap_or_else(|| "open".into()))
        .bind(input.priority.unwrap_or_else(|| "medium".into()))
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
            push_update_assignment(&mut builder, &mut needs_separator, "status", status);
            changed += 1;
        }

        if let Some(priority) = input.priority.as_ref() {
            push_update_assignment(&mut builder, &mut needs_separator, "priority", priority);
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

    pub async fn create_job(
        &self,
        tenant_id: Option<Uuid>,
        job_type: &str,
        payload: Value,
        max_attempts: i32,
    ) -> AppResult<BackgroundJobRecord> {
        sqlx::query_as::<_, BackgroundJobRecord>(
            r#"
            INSERT INTO background_jobs (
                id,
                tenant_id,
                job_type,
                status,
                attempts,
                max_attempts,
                scheduled_at,
                payload
            )
            VALUES ($1, $2, $3, 'queued', 0, $4, $5, $6)
            RETURNING id, tenant_id, job_type, status, attempts, max_attempts, scheduled_at,
                      started_at, finished_at, payload, result_payload, last_error
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(job_type)
        .bind(max_attempts)
        .bind(Utc::now())
        .bind(payload)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_job(&self, job_id: Uuid) -> AppResult<Option<BackgroundJobRecord>> {
        sqlx::query_as::<_, BackgroundJobRecord>(
            r#"
            SELECT id, tenant_id, job_type, status, attempts, max_attempts, scheduled_at,
                   started_at, finished_at, payload, result_payload, last_error
            FROM background_jobs
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_ready_job_ids(&self, limit: i64) -> AppResult<Vec<Uuid>> {
        sqlx::query_scalar(
            r#"
            SELECT id
            FROM background_jobs
            WHERE status = 'queued' AND scheduled_at <= now()
            ORDER BY scheduled_at ASC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn mark_job_running(&self, job_id: Uuid) -> AppResult<Option<BackgroundJobRecord>> {
        sqlx::query_as::<_, BackgroundJobRecord>(
            r#"
            UPDATE background_jobs
            SET status = 'running',
                attempts = attempts + 1,
                started_at = now(),
                last_error = NULL
            WHERE id = $1
              AND status = 'queued'
              AND scheduled_at <= now()
            RETURNING id, tenant_id, job_type, status, attempts, max_attempts, scheduled_at,
                      started_at, finished_at, payload, result_payload, last_error
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn complete_job(&self, job_id: Uuid, result_payload: Value) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE background_jobs
            SET status = $2, finished_at = now(), result_payload = $3
            WHERE id = $1
            "#,
        )
        .bind(job_id)
        .bind(JOB_STATUS_COMPLETED)
        .bind(result_payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_job(&self, job: &BackgroundJobRecord, message: &str) -> AppResult<()> {
        let status = if job.attempts >= job.max_attempts {
            JOB_STATUS_DEAD_LETTER
        } else {
            JOB_STATUS_QUEUED
        };

        let next_time = if status == JOB_STATUS_DEAD_LETTER {
            Utc::now()
        } else {
            let seconds = 2_i64.pow(job.attempts.clamp(1, 8) as u32);
            Utc::now() + ChronoDuration::seconds(seconds)
        };

        sqlx::query(
            r#"
            UPDATE background_jobs
            SET status = $2,
                scheduled_at = $3,
                finished_at = CASE WHEN $2 = 'dead_letter' THEN now() ELSE NULL END,
                last_error = $4
            WHERE id = $1
            "#,
        )
        .bind(job.id)
        .bind(status)
        .bind(next_time)
        .bind(message)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn ensure_due_reminder_job(
        &self,
        tenant_id: Option<Uuid>,
        max_attempts: i32,
    ) -> AppResult<Option<BackgroundJobRecord>> {
        let existing: Option<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id
            FROM background_jobs
            WHERE job_type = $1
              AND (
                    (tenant_id = $2)
                    OR (tenant_id IS NULL AND $2 IS NULL)
                  )
              AND status IN ('queued', 'running')
            ORDER BY scheduled_at DESC
            LIMIT 1
            "#,
        )
        .bind(JOB_TYPE_DUE_REMINDER_SWEEP)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await?;

        if existing.is_some() {
            return Ok(None);
        }

        self.create_job(
            tenant_id,
            JOB_TYPE_DUE_REMINDER_SWEEP,
            json!({ "tenant_id": tenant_id }),
            max_attempts,
        )
        .await
        .map(Some)
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
        builder.push_bind(status);
    }

    if let Some(priority) = filters.priority.as_ref() {
        builder.push(" AND priority = ");
        builder.push_bind(priority);
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
