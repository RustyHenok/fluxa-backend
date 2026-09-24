use chrono::Utc;
use metrics::counter;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;
use uuid::Uuid;

use crate::domain::{
    BackgroundJobRecord, ExportFormat, JobResponse, JobResultResponse, JobStatus, JobType,
    NewNotification, TaskFilters, TaskRecord, TaskResponse,
};
use crate::error::{AppError, AppResult};
use crate::notify::{KIND_TASK_DUE_SOON, KIND_TASK_OVERDUE};
use crate::pagination::Cursor;
use crate::services::tasks;
use crate::state::AppState;
use crate::storage::ArtifactStore;

const EXPORT_CHUNK_SIZE: usize = 500;
const REMINDER_CANDIDATE_LIMIT: i64 = 500;

#[derive(Debug, Deserialize)]
struct ExportJobPayload {
    tenant_id: Uuid,
    requested_by: Uuid,
    filters: TaskFilters,
    #[serde(default)]
    format: ExportFormat,
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
    format: ExportFormat,
) -> AppResult<BackgroundJobRecord> {
    let job = state
        .db
        .create_job(
            Some(tenant_id),
            JobType::TaskExport.as_str(),
            json!({
                "tenant_id": tenant_id,
                "requested_by": requested_by,
                "filters": filters.export_payload(),
                "format": format,
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

/// Enqueues a retention sweep when none is pending and the cadence window has
/// elapsed since the last one was scheduled.
pub async fn enqueue_retention_sweep(state: &AppState) -> AppResult<Option<BackgroundJobRecord>> {
    let maybe_job = state
        .db
        .ensure_retention_job(
            state.config.retention_sweep_interval_hours,
            state.config.max_job_attempts,
        )
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

pub async fn get_tenant_job_result(
    state: &AppState,
    job_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<JobResultResponse> {
    let job = get_tenant_job(state, job_id, tenant_id).await?;

    match job.parsed_status()? {
        JobStatus::Completed => JobResultResponse::try_from(&job),
        JobStatus::Queued | JobStatus::Running => {
            Err(AppError::Conflict("job result is not ready".into()))
        }
        JobStatus::DeadLetter => Err(AppError::Conflict(
            "job did not complete successfully".into(),
        )),
    }
}

pub async fn dispatch_ready_jobs(state: &AppState, limit: i64) -> AppResult<()> {
    let job_ids = state.db.list_ready_job_ids(limit).await?;
    for job_id in job_ids {
        state.cache.enqueue_job(job_id).await?;
    }

    Ok(())
}

/// Recovers jobs stuck in `running` after their lease expired, either
/// requeueing them or moving them to `dead_letter` once attempts are spent.
pub async fn reap_stale_jobs(state: &AppState) -> AppResult<()> {
    let reclaimed = state
        .db
        .requeue_stale_jobs(state.config.job_lease())
        .await?;

    for (job_id, status) in reclaimed {
        warn!("reclaimed stale job {job_id} -> {status}");
        counter!("jobs_reclaimed_total", "status" => status).increment(1);
    }

    Ok(())
}

pub async fn process_job(state: &AppState, job_id: Uuid) -> AppResult<()> {
    let Some(job) = state.db.mark_job_running(job_id).await? else {
        return Ok(());
    };

    let outcome = match job.parsed_job_type()? {
        JobType::TaskExport => process_export_job(state, &job).await,
        JobType::DueReminderSweep => process_due_reminder_job(state, &job).await,
        JobType::RetentionSweep => process_retention_job(state).await,
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
    serde_json::to_value(JobResponse::try_from(job)?)
        .map_err(|error| AppError::internal(format!("failed to serialize job: {error}")))
}

async fn process_export_job(
    state: &AppState,
    job: &BackgroundJobRecord,
) -> AppResult<serde_json::Value> {
    let payload: ExportJobPayload = serde_json::from_value(job.payload.clone())
        .map_err(|error| AppError::internal(format!("invalid export job payload: {error}")))?;

    let mut all_tasks: Vec<TaskRecord> = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let chunk = tasks::export_tasks(
            state,
            payload.tenant_id,
            &payload.filters,
            cursor.as_ref(),
            EXPORT_CHUNK_SIZE,
        )
        .await?;
        let full_chunk = chunk.len() == EXPORT_CHUNK_SIZE;
        cursor = chunk.last().map(|task| Cursor {
            updated_at: task.updated_at,
            id: task.id,
        });
        all_tasks.extend(chunk);
        if !full_chunk {
            break;
        }
    }

    let responses = all_tasks
        .iter()
        .map(TaskResponse::try_from)
        .collect::<AppResult<Vec<_>>>()?;

    let bytes = match payload.format {
        ExportFormat::Json => serde_json::to_vec(&json!({ "tasks": responses }))
            .map_err(|error| AppError::internal(format!("failed to encode export: {error}")))?,
        ExportFormat::Csv => render_tasks_csv(&responses).into_bytes(),
    };

    let key = format!(
        "{}/{}.{}",
        payload.tenant_id,
        job.id,
        payload.format.extension()
    );
    state.storage.put(&key, &bytes).await?;

    Ok(json!({
        "requested_by": payload.requested_by,
        "generated_at": Utc::now(),
        "task_count": responses.len(),
        "format": payload.format,
        "artifact": {
            "key": key,
            "content_type": payload.format.content_type(),
            "size_bytes": bytes.len(),
            "download_path": format!("/v1/jobs/{}/artifact", job.id),
        },
    }))
}

/// Renders tasks as RFC 4180 CSV with a header row.
fn render_tasks_csv(tasks: &[TaskResponse]) -> String {
    let mut out = String::from(
        "id,project_id,title,description,status,priority,assignee_id,due_at,created_by,updated_by,created_at,updated_at\r\n",
    );

    for task in tasks {
        let fields = [
            task.id.to_string(),
            task.project_id.map(|id| id.to_string()).unwrap_or_default(),
            task.title.clone(),
            task.description.clone().unwrap_or_default(),
            task.status.to_string(),
            task.priority.to_string(),
            task.assignee_id
                .map(|id| id.to_string())
                .unwrap_or_default(),
            task.due_at.map(|at| at.to_rfc3339()).unwrap_or_default(),
            task.created_by.to_string(),
            task.updated_by.to_string(),
            task.created_at.to_rfc3339(),
            task.updated_at.to_rfc3339(),
        ];
        let row = fields
            .iter()
            .map(|field| csv_escape(field))
            .collect::<Vec<_>>()
            .join(",");
        out.push_str(&row);
        out.push_str("\r\n");
    }

    out
}

fn csv_escape(field: &str) -> String {
    if field.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

/// Runs all retention purges and reports per-table deletion counts.
async fn process_retention_job(state: &AppState) -> AppResult<serde_json::Value> {
    let refresh_tokens = state
        .db
        .purge_stale_refresh_tokens(state.config.refresh_token_retention_days)
        .await?;
    let jobs = state
        .db
        .purge_terminal_jobs(state.config.job_retention_days)
        .await?;
    let notifications = state
        .db
        .purge_terminal_notifications(state.config.notification_retention_days)
        .await?;
    let audit_events = state
        .db
        .purge_old_audit_events(state.config.audit_retention_days)
        .await?;

    counter!("retention_rows_purged_total", "table" => "refresh_tokens").increment(refresh_tokens);
    counter!("retention_rows_purged_total", "table" => "background_jobs").increment(jobs);
    counter!("retention_rows_purged_total", "table" => "notifications").increment(notifications);
    counter!("retention_rows_purged_total", "table" => "audit_log").increment(audit_events);

    Ok(json!({
        "generated_at": Utc::now(),
        "refresh_tokens_purged": refresh_tokens,
        "jobs_purged": jobs,
        "notifications_purged": notifications,
        "audit_events_purged": audit_events,
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
    let notified = enqueue_due_reminder_notifications(state, payload.tenant_id).await?;

    Ok(json!({
        "generated_at": Utc::now(),
        "tenant_id": payload.tenant_id,
        "reminder_count": reminders,
        "notification_count": notified,
    }))
}

/// Writes per-assignee due-soon/overdue rows into the notifications outbox,
/// deduplicated per task, user, and dedupe window so repeated sweeps do not
/// re-notify.
async fn enqueue_due_reminder_notifications(
    state: &AppState,
    tenant_id: Option<Uuid>,
) -> AppResult<usize> {
    let candidates = state
        .db
        .list_due_reminder_candidates(
            state.config.reminder_due_soon_hours,
            REMINDER_CANDIDATE_LIMIT,
            tenant_id,
        )
        .await?;

    let now = Utc::now();
    let dedupe_window_seconds = state.config.reminder_dedupe_ttl_hours * 3600;
    let bucket = now.timestamp() / dedupe_window_seconds.max(1);

    let mut enqueued = 0usize;
    for candidate in &candidates {
        let overdue = candidate.due_at.is_some_and(|due_at| due_at <= now);
        let kind = if overdue {
            KIND_TASK_OVERDUE
        } else {
            KIND_TASK_DUE_SOON
        };
        let dedupe_key = format!(
            "{kind}:{}:{}:{bucket}",
            candidate.task_id, candidate.assignee_id
        );

        let inserted = state
            .db
            .enqueue_notification(
                &NewNotification {
                    tenant_id: Some(candidate.tenant_id),
                    user_id: Some(candidate.assignee_id),
                    kind: kind.into(),
                    recipient: candidate.email.clone(),
                    payload: json!({
                        "task_id": candidate.task_id,
                        "title": candidate.title,
                        "due_at": candidate.due_at.map(|at| at.to_rfc3339()),
                    }),
                    dedupe_key: Some(dedupe_key),
                },
                state.config.max_job_attempts,
            )
            .await?;
        if inserted {
            enqueued += 1;
        }
    }

    Ok(enqueued)
}
