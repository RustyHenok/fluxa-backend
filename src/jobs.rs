use chrono::Utc;
use metrics::counter;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::watch;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::domain::{
    BackgroundJobRecord, JOB_TYPE_DUE_REMINDER_SWEEP, JOB_TYPE_TASK_EXPORT, TaskFilters,
    TaskResponse,
};
use crate::error::{AppError, AppResult};
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

pub async fn run_worker(state: AppState, shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let dispatch_state = state.clone();
    let scheduler_state = state.clone();
    let processor_state = state.clone();

    let dispatch_shutdown = shutdown.clone();
    let scheduler_shutdown = shutdown.clone();
    let processor_shutdown = shutdown.clone();

    tokio::try_join!(
        dispatch_ready_jobs_loop(dispatch_state, dispatch_shutdown),
        schedule_due_reminders_loop(scheduler_state, scheduler_shutdown),
        process_jobs_loop(processor_state, processor_shutdown),
    )?;

    Ok(())
}

async fn dispatch_ready_jobs_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.worker_dispatch_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("job dispatcher shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                let job_ids = state.db.list_ready_job_ids(100).await?;
                for job_id in job_ids {
                    if let Err(error) = state.cache.enqueue_job(job_id).await {
                        warn!("failed to enqueue job {job_id}: {error}");
                    }
                }
            }
        }
    }
}

async fn schedule_due_reminders_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.worker_scheduler_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("due reminder scheduler shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                if let Some(job) = state
                    .db
                    .ensure_due_reminder_job(None, state.config.max_job_attempts)
                    .await?
                {
                    if let Err(error) = state.cache.enqueue_job(job.id).await {
                        warn!("failed to enqueue scheduled reminder job {}: {error}", job.id);
                    }
                }
            }
        }
    }
}

async fn process_jobs_loop(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("job processor shutting down");
                return Ok(());
            }
            result = state.cache.dequeue_job(state.config.job_queue_block_timeout_seconds) => {
                match result {
                    Ok(Some(job_id)) => {
                        if let Err(error) = process_job(&state, job_id).await {
                            error!("job processing failed for {job_id}: {error}");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => warn!("failed to dequeue job: {error}"),
                }
            }
        }
    }
}

async fn process_job(state: &AppState, job_id: Uuid) -> AppResult<()> {
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

async fn process_export_job(
    state: &AppState,
    job: &BackgroundJobRecord,
) -> AppResult<serde_json::Value> {
    let payload: ExportJobPayload = serde_json::from_value(job.payload.clone())
        .map_err(|error| AppError::internal(format!("invalid export job payload: {error}")))?;

    let tasks = state
        .db
        .export_tasks(payload.tenant_id, &payload.filters, 1_000)
        .await?;

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
    let reminders = state.db.record_due_reminders(payload.tenant_id).await?;

    Ok(json!({
        "generated_at": Utc::now(),
        "tenant_id": payload.tenant_id,
        "reminder_count": reminders,
    }))
}
