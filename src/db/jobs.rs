use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::Database;
use crate::domain::{
    BackgroundJobRecord, JOB_STATUS_COMPLETED, JOB_STATUS_DEAD_LETTER, JOB_TYPE_DUE_REMINDER_SWEEP,
};
use crate::error::{AppError, AppResult};

impl Database {
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
            "queued"
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
}
