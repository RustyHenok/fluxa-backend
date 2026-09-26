use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use tracing::warn;
use uuid::Uuid;

use crate::domain::{
    CreateTaskInput, DashboardSummary, PaginatedTaskAudit, PaginatedTasks, TaskFilters, TaskRecord,
    TaskResponse, TaskStatus, UpdateTaskInput, WEBHOOK_EVENT_TASK_ARCHIVED,
    WEBHOOK_EVENT_TASK_CREATED, WEBHOOK_EVENT_TASK_RESTORED, WEBHOOK_EVENT_TASK_STATUS_UPDATED,
    WEBHOOK_EVENT_TASK_UPDATED, normalize_bulk_task_ids,
};
use crate::error::{AppError, AppResult};
use crate::pagination::{AuditCursor, Cursor};
use crate::services::webhooks as webhook_service;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPage {
    pub data: Vec<TaskResponse>,
    pub next_cursor: Option<String>,
}

pub async fn list_tasks_cached(
    state: &AppState,
    tenant_id: Uuid,
    filters: &TaskFilters,
    cursor_token: Option<&str>,
    cursor: Option<&Cursor>,
    limit: usize,
) -> AppResult<TaskPage> {
    let version = state.cache.tenant_cache_version(tenant_id).await?;
    let cache_payload = json!({
        "tenant_id": tenant_id,
        "limit": limit,
        "cursor": cursor_token,
        "filters": filters,
    });
    let cache_key = state
        .cache
        .task_list_cache_key(tenant_id, version, &cache_payload)?;

    if let Some(cached) = state.cache.get_json::<TaskPage>(&cache_key).await? {
        return Ok(cached);
    }

    let tasks = list_tasks(state, tenant_id, filters, cursor, limit).await?;
    let response = TaskPage {
        data: tasks
            .tasks
            .iter()
            .map(TaskResponse::try_from)
            .collect::<AppResult<Vec<_>>>()?,
        next_cursor: tasks.next_cursor.map(|value| value.encode()).transpose()?,
    };

    state
        .cache
        .set_json(&cache_key, &response, state.config.cache_ttl())
        .await?;

    Ok(response)
}

pub async fn get_task_cached(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
) -> AppResult<TaskResponse> {
    let version = state.cache.tenant_cache_version(tenant_id).await?;
    let cache_key = state
        .cache
        .task_detail_cache_key(tenant_id, version, task_id);

    if let Some(cached) = state.cache.get_json::<TaskResponse>(&cache_key).await? {
        return Ok(cached);
    }

    let task = get_task(state, tenant_id, task_id).await?;
    let response = TaskResponse::try_from(&task)?;
    state
        .cache
        .set_json(&cache_key, &response, state.config.cache_ttl())
        .await?;

    Ok(response)
}

pub async fn list_tasks(
    state: &AppState,
    tenant_id: Uuid,
    filters: &TaskFilters,
    cursor: Option<&Cursor>,
    limit: usize,
) -> AppResult<PaginatedTasks> {
    state.db.list_tasks(tenant_id, filters, cursor, limit).await
}

pub async fn dashboard_summary(state: &AppState, tenant_id: Uuid) -> AppResult<DashboardSummary> {
    state.db.dashboard_summary(tenant_id).await
}

pub async fn list_task_audit(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    cursor: Option<&AuditCursor>,
    limit: usize,
) -> AppResult<PaginatedTaskAudit> {
    get_task(state, tenant_id, task_id).await?;
    state
        .db
        .list_task_audit(tenant_id, task_id, cursor, limit)
        .await
}

pub async fn get_task(state: &AppState, tenant_id: Uuid, task_id: Uuid) -> AppResult<TaskRecord> {
    state
        .db
        .get_task(tenant_id, task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("task not found".into()))
}

pub async fn create_task(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    input: CreateTaskInput,
) -> AppResult<TaskRecord> {
    ensure_project_belongs_to_tenant(state, tenant_id, input.project_id).await?;
    let task = state.db.create_task(tenant_id, actor_id, input).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    emit_task_webhook(state, tenant_id, WEBHOOK_EVENT_TASK_CREATED, &task).await;
    Ok(task)
}

pub async fn update_task(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    input: UpdateTaskInput,
) -> AppResult<TaskRecord> {
    if let Some(project_id) = input.project_id {
        ensure_project_belongs_to_tenant(state, tenant_id, project_id).await?;
    }
    let task = state
        .db
        .update_task(tenant_id, task_id, actor_id, input)
        .await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    emit_task_webhook(state, tenant_id, WEBHOOK_EVENT_TASK_UPDATED, &task).await;
    Ok(task)
}

pub async fn bulk_update_task_status(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    task_ids: Vec<Uuid>,
    status: TaskStatus,
) -> AppResult<Vec<TaskRecord>> {
    let task_ids = normalize_bulk_task_ids(task_ids)?;
    let mut tasks = state
        .db
        .bulk_update_task_status(tenant_id, actor_id, &task_ids, status)
        .await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;

    // Return records in the order the ids were requested.
    let positions: HashMap<Uuid, usize> = task_ids
        .iter()
        .enumerate()
        .map(|(index, task_id)| (*task_id, index))
        .collect();
    tasks.sort_by_key(|task| positions.get(&task.id).copied().unwrap_or(usize::MAX));
    for task in &tasks {
        emit_task_webhook(state, tenant_id, WEBHOOK_EVENT_TASK_STATUS_UPDATED, task).await;
    }
    Ok(tasks)
}

pub async fn archive_task(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
) -> AppResult<()> {
    let task = state.db.archive_task(tenant_id, task_id, actor_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    emit_task_webhook(state, tenant_id, WEBHOOK_EVENT_TASK_ARCHIVED, &task).await;
    Ok(())
}

pub async fn restore_task(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
) -> AppResult<TaskRecord> {
    let task = state.db.restore_task(tenant_id, task_id, actor_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    emit_task_webhook(state, tenant_id, WEBHOOK_EVENT_TASK_RESTORED, &task).await;
    Ok(task)
}

pub async fn export_tasks(
    state: &AppState,
    tenant_id: Uuid,
    filters: &TaskFilters,
    cursor: Option<&Cursor>,
    limit: usize,
) -> AppResult<Vec<TaskRecord>> {
    state
        .db
        .export_tasks(tenant_id, filters, cursor, limit)
        .await
}

pub async fn record_due_reminders(state: &AppState, tenant_id: Option<Uuid>) -> AppResult<usize> {
    state.db.record_due_reminders(tenant_id).await
}

async fn ensure_project_belongs_to_tenant(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Option<Uuid>,
) -> AppResult<()> {
    let Some(project_id) = project_id else {
        return Ok(());
    };

    state
        .db
        .get_project(tenant_id, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project not found".into()))?;

    Ok(())
}

/// Serializes the task and queues webhook deliveries. Failures never break
/// the primary task operation.
async fn emit_task_webhook(state: &AppState, tenant_id: Uuid, event_type: &str, task: &TaskRecord) {
    let payload = TaskResponse::try_from(task).and_then(|response| {
        serde_json::to_value(response)
            .map_err(|error| AppError::internal(format!("failed to serialize task: {error}")))
    });
    match payload {
        Ok(payload) => {
            webhook_service::emit_task_event(state, tenant_id, event_type, &payload).await;
        }
        Err(error) => {
            warn!(
                %tenant_id,
                event_type,
                "failed to build webhook payload: {error}"
            );
        }
    }
}
