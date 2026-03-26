use chrono::Utc;
use metrics::counter;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::domain::{
    BackgroundJobRecord, JOB_TYPE_DUE_REMINDER_SWEEP, JOB_TYPE_TASK_EXPORT, JobResponse,
    TaskFilters, TaskResponse,
};
use crate::error::{AppError, AppResult};
use crate::services::tasks;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
struct ExportJobPayload {
    tenant_id: Uuid,
    requested_by: Uuid,
    filters: TaskFilters,
}

#[derive(Debug, Deserialize)]
struct DueReminderPayload {
    tenant_id: Option<Uuid>,
}

pub async fn create_export_job(
    state: &AppState,
    tenant_id: Uuid,
    requested_by: Uuid,
    filters: &TaskFilters,
) -> AppResult<BackgroundJobRecord> {
    let job = state
        .db
        .create_job(
            Some(tenant_id),
            JOB_TYPE_TASK_EXPORT,
            json!({
                "tenant_id": tenant_id,
                "requested_by": requested_by,
                "filters": filters.export_payload(),
            }),
            state.config.max_job_attempts,
        )
        .await?;

    state.cache.enqueue_job(job.id).await?;
    Ok(job)
}

pub async fn enqueue_due_reminder_sweep(
    state: &AppState,
    tenant_id: Option<Uuid>,
) -> AppResult<Option<BackgroundJobRecord>> {
    let maybe_job = state
        .db
        .ensure_due_reminder_job(tenant_id, state.config.max_job_attempts)
        .await?;

    if let Some(job) = maybe_job.as_ref() {
        state.cache.enqueue_job(job.id).await?;
    }

    Ok(maybe_job)
}

pub async fn get_job(state: &AppState, job_id: Uuid) -> AppResult<Option<BackgroundJobRecord>> {
    state.db.get_job(job_id).await
}

pub async fn get_tenant_job(
    state: &AppState,
    job_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<BackgroundJobRecord> {
    let job = get_job(state, job_id)
        .await?
        .ok_or_else(|| AppError::NotFound("job not found".into()))?;

    if job.tenant_id != Some(tenant_id) {
        return Err(AppError::NotFound("job not found".into()));
    }

    Ok(job)
}

pub async fn dispatch_ready_jobs(state: &AppState, limit: i64) -> AppResult<()> {
    let job_ids = state.db.list_ready_job_ids(limit).await?;
    for job_id in job_ids {
        state.cache.enqueue_job(job_id).await?;
    }

    Ok(())
}

pub async fn process_job(state: &AppState, job_id: Uuid) -> AppResult<()> {
    let Some(job) = state.db.mark_job_running(job_id).await? else {
        return Ok(());
    };

    let outcome = match job.job_type.as_str() {
        JOB_TYPE_TASK_EXPORT => process_export_job(state, &job).await,
        JOB_TYPE_DUE_REMINDER_SWEEP => process_due_reminder_job(state, &job).await,
        other => Err(AppError::internal(format!("unsupported job type: {other}"))),
    };

    match outcome {
        Ok(result_payload) => {
            state.db.complete_job(job.id, result_payload).await?;
            counter!("jobs_completed_total", "job_type" => job.job_type.clone()).increment(1);
        }
        Err(error) => {
            state.db.fail_job(&job, &error.to_string()).await?;
            counter!("jobs_failed_total", "job_type" => job.job_type.clone()).increment(1);
        }
    }

    Ok(())
}

pub fn job_response_value(job: &BackgroundJobRecord) -> AppResult<Value> {
    serde_json::to_value(JobResponse::from(job))
        .map_err(|error| AppError::internal(format!("failed to serialize job: {error}")))
}

async fn process_export_job(
    state: &AppState,
    job: &BackgroundJobRecord,
) -> AppResult<serde_json::Value> {
    let payload: ExportJobPayload = serde_json::from_value(job.payload.clone())
        .map_err(|error| AppError::internal(format!("invalid export job payload: {error}")))?;

    let tasks = tasks::export_tasks(state, payload.tenant_id, &payload.filters, 1_000).await?;

    Ok(json!({
        "requested_by": payload.requested_by,
        "generated_at": Utc::now(),
        "task_count": tasks.len(),
        "tasks": tasks.iter().map(TaskResponse::from).collect::<Vec<_>>(),
    }))
}

async fn process_due_reminder_job(
    state: &AppState,
    job: &BackgroundJobRecord,
) -> AppResult<serde_json::Value> {
    let payload: DueReminderPayload =
        serde_json::from_value(job.payload.clone()).unwrap_or(DueReminderPayload {
            tenant_id: job.tenant_id,
        });
    let reminders = tasks::record_due_reminders(state, payload.tenant_id).await?;

    Ok(json!({
        "generated_at": Utc::now(),
        "tenant_id": payload.tenant_id,
        "reminder_count": reminders,
    }))
}
