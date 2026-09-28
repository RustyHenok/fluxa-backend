use serde_json::json;
use uuid::Uuid;

use crate::domain::{CreateLabelInput, LabelRecord, UpdateLabelInput, validate_task_label_ids};
use crate::error::AppResult;
use crate::services::{audit, tasks as task_service};
use crate::state::AppState;

pub async fn list_labels(state: &AppState, tenant_id: Uuid) -> AppResult<Vec<LabelRecord>> {
    state.db.list_labels(tenant_id).await
}

pub async fn create_label(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    input: CreateLabelInput,
) -> AppResult<LabelRecord> {
    let label = state.db.create_label(tenant_id, actor_id, input).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "label",
        Some(label.id),
        "label.created",
        json!({ "name": label.name, "color": label.color }),
    )
    .await;
    Ok(label)
}

pub async fn update_label(
    state: &AppState,
    tenant_id: Uuid,
    label_id: Uuid,
    actor_id: Uuid,
    input: UpdateLabelInput,
) -> AppResult<LabelRecord> {
    let label = state
        .db
        .update_label(tenant_id, label_id, actor_id, input)
        .await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "label",
        Some(label.id),
        "label.updated",
        json!({ "name": label.name, "color": label.color }),
    )
    .await;
    Ok(label)
}

pub async fn delete_label(
    state: &AppState,
    tenant_id: Uuid,
    label_id: Uuid,
    actor_id: Uuid,
) -> AppResult<()> {
    let label = state.db.delete_label(tenant_id, label_id).await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_id),
        "label",
        Some(label_id),
        "label.deleted",
        json!({ "name": label.name }),
    )
    .await;
    Ok(())
}

pub async fn list_task_labels(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
) -> AppResult<Vec<LabelRecord>> {
    task_service::get_task(state, tenant_id, task_id).await?;
    state.db.list_task_labels(tenant_id, task_id).await
}

pub async fn set_task_labels(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    label_ids: Vec<Uuid>,
) -> AppResult<Vec<LabelRecord>> {
    let label_ids = validate_task_label_ids(&label_ids)?;
    task_service::get_task(state, tenant_id, task_id).await?;
    let labels = state
        .db
        .set_task_labels(tenant_id, task_id, actor_id, &label_ids)
        .await?;
    state.cache.bump_tenant_cache_version(tenant_id).await?;
    Ok(labels)
}
