use serde_json::json;
use uuid::Uuid;

use crate::domain::{CreateProjectInput, ProjectRecord, ProjectSummary, UpdateProjectInput};
use crate::error::{AppError, AppResult};
use crate::services::audit;
use crate::state::AppState;

pub async fn list_projects(state: &AppState, tenant_id: Uuid) -> AppResult<Vec<ProjectRecord>> {
    state.db.list_projects(tenant_id).await
}

pub async fn get_project(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
) -> AppResult<ProjectRecord> {
    state
        .db
        .get_project(tenant_id, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project not found".into()))
}

pub async fn create_project(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    input: CreateProjectInput,
) -> AppResult<ProjectRecord> {
    let project = state.db.create_project(tenant_id, actor_id, input).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "project",
        Some(project.id),
        "project.created",
        json!({ "name": project.name }),
    )
    .await;
    Ok(project)
}

pub async fn project_summary(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
) -> AppResult<ProjectSummary> {
    state
        .db
        .project_summary(tenant_id, project_id)
        .await?
        .ok_or_else(|| AppError::NotFound("project not found".into()))
}

pub async fn update_project(
    state: &AppState,
    tenant_id: Uuid,
    project_id: Uuid,
    actor_id: Uuid,
    input: UpdateProjectInput,
) -> AppResult<ProjectRecord> {
    let project = state
        .db
        .update_project(tenant_id, project_id, actor_id, input)
        .await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "project",
        Some(project.id),
        "project.updated",
        json!({ "name": project.name }),
    )
    .await;
    Ok(project)
}

pub async fn delete_project(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    project_id: Uuid,
) -> AppResult<()> {
    state.db.delete_project(tenant_id, project_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "project",
        Some(project_id),
        "project.deleted",
        json!({}),
    )
    .await;
    Ok(())
}
