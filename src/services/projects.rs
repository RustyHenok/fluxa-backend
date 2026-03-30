use uuid::Uuid;

use crate::domain::{CreateProjectInput, ProjectRecord, UpdateProjectInput};
use crate::error::{AppError, AppResult};
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
    Ok(project)
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
    Ok(project)
}

pub async fn delete_project(state: &AppState, tenant_id: Uuid, project_id: Uuid) -> AppResult<()> {
    state.db.delete_project(tenant_id, project_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    Ok(())
}
