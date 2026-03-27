use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::domain::{
    CreateTaskInput, DashboardSummary, PaginatedTasks, TaskFilters, TaskRecord, TaskResponse,
    UpdateTaskInput,
};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;
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
    let task = state.db.create_task(tenant_id, actor_id, input).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    Ok(task)
}

pub async fn update_task(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    input: UpdateTaskInput,
) -> AppResult<TaskRecord> {
    let task = state
        .db
        .update_task(tenant_id, task_id, actor_id, input)
        .await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    Ok(task)
}

pub async fn delete_task(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
) -> AppResult<()> {
    state.db.delete_task(tenant_id, task_id, actor_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    Ok(())
}

pub async fn export_tasks(
    state: &AppState,
    tenant_id: Uuid,
    filters: &TaskFilters,
    limit: usize,
) -> AppResult<Vec<TaskRecord>> {
    state.db.export_tasks(tenant_id, filters, limit).await
}

pub async fn record_due_reminders(state: &AppState, tenant_id: Option<Uuid>) -> AppResult<usize> {
    state.db.record_due_reminders(tenant_id).await
}
