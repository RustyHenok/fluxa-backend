use uuid::Uuid;

use crate::domain::{AttachmentRecord, MAX_ATTACHMENTS_PER_TASK, MembershipRole};
use crate::error::{AppError, AppResult};
use crate::services::tasks as task_service;
use crate::state::AppState;
use crate::storage::ArtifactStore;

pub async fn list_attachments(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
) -> AppResult<Vec<AttachmentRecord>> {
    task_service::get_task(state, tenant_id, task_id).await?;
    state.db.list_task_attachments(tenant_id, task_id).await
}

pub async fn upload_attachment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    actor_id: Uuid,
    file_name: String,
    content_type: String,
    bytes: &[u8],
) -> AppResult<AttachmentRecord> {
    task_service::get_task(state, tenant_id, task_id).await?;

    let count = state.db.count_task_attachments(tenant_id, task_id).await?;
    if count >= MAX_ATTACHMENTS_PER_TASK {
        return Err(AppError::Validation(format!(
            "a task can have at most {MAX_ATTACHMENTS_PER_TASK} attachments"
        )));
    }

    let attachment_id = Uuid::new_v4();
    let storage_key = format!("attachments/{tenant_id}/{task_id}/{attachment_id}");
    state.storage.put(&storage_key, bytes).await?;

    match state
        .db
        .create_attachment(
            attachment_id,
            tenant_id,
            task_id,
            actor_id,
            &file_name,
            &content_type,
            bytes.len() as i64,
            &storage_key,
        )
        .await
    {
        Ok(attachment) => Ok(attachment),
        Err(error) => {
            if let Err(cleanup_error) = state.storage.delete(&storage_key).await {
                tracing::warn!(
                    "failed to clean up attachment blob after insert failure: {cleanup_error}"
                );
            }
            Err(error)
        }
    }
}

pub async fn download_attachment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    attachment_id: Uuid,
) -> AppResult<(AttachmentRecord, Vec<u8>)> {
    task_service::get_task(state, tenant_id, task_id).await?;
    let attachment = state
        .db
        .get_attachment(tenant_id, task_id, attachment_id)
        .await?;
    let bytes = state
        .storage
        .get(&attachment.storage_key)
        .await?
        .ok_or_else(|| AppError::NotFound("attachment content is no longer available".into()))?;
    Ok((attachment, bytes))
}

pub async fn delete_attachment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    attachment_id: Uuid,
    actor_id: Uuid,
    role: MembershipRole,
) -> AppResult<()> {
    task_service::get_task(state, tenant_id, task_id).await?;
    let attachment = state
        .db
        .get_attachment(tenant_id, task_id, attachment_id)
        .await?;
    let is_uploader = attachment.uploaded_by == actor_id;
    let is_admin = matches!(role, MembershipRole::Owner | MembershipRole::Admin);
    if !is_uploader && !is_admin {
        return Err(AppError::Forbidden(
            "only the uploader or an owner/admin can delete an attachment".into(),
        ));
    }

    let deleted = state
        .db
        .delete_attachment(tenant_id, task_id, attachment_id, actor_id)
        .await?;
    if let Err(error) = state.storage.delete(&deleted.storage_key).await {
        tracing::warn!("failed to delete attachment blob: {error}");
    }
    Ok(())
}
