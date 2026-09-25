use serde_json::json;
use uuid::Uuid;

use crate::domain::{CommentRecord, MembershipRole, NewNotification, PaginatedComments};
use crate::error::{AppError, AppResult};
use crate::notify::KIND_TASK_COMMENTED;
use crate::pagination::AuditCursor;
use crate::services::tasks as task_service;
use crate::state::AppState;

pub async fn list_comments(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    cursor: Option<&AuditCursor>,
    limit: usize,
) -> AppResult<PaginatedComments> {
    task_service::get_task(state, tenant_id, task_id).await?;
    state
        .db
        .list_task_comments(tenant_id, task_id, cursor, limit)
        .await
}

pub async fn create_comment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    author_id: Uuid,
    body: String,
) -> AppResult<CommentRecord> {
    let task = task_service::get_task(state, tenant_id, task_id).await?;
    let comment = state
        .db
        .create_comment(tenant_id, task_id, author_id, body)
        .await?;

    if let Some(assignee_id) = task.assignee_id
        && assignee_id != author_id
    {
        match state.db.get_user_by_id(assignee_id).await {
            Ok(assignee) => {
                let enqueued = state
                    .db
                    .enqueue_notification(
                        &NewNotification {
                            tenant_id: Some(tenant_id),
                            user_id: Some(assignee_id),
                            kind: KIND_TASK_COMMENTED.into(),
                            recipient: assignee.email,
                            payload: json!({
                                "task_id": task_id,
                                "title": task.title,
                                "comment_id": comment.id,
                                "body": comment.body,
                            }),
                            dedupe_key: None,
                        },
                        state.config.max_job_attempts,
                    )
                    .await;
                if let Err(error) = enqueued {
                    tracing::warn!("failed to enqueue comment notification: {error}");
                }
            }
            Err(error) => {
                tracing::warn!("failed to load assignee for comment notification: {error}");
            }
        }
    }

    Ok(comment)
}

pub async fn update_comment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    comment_id: Uuid,
    actor_id: Uuid,
    body: String,
) -> AppResult<CommentRecord> {
    task_service::get_task(state, tenant_id, task_id).await?;
    let comment = state.db.get_comment(tenant_id, task_id, comment_id).await?;
    if comment.author_id != actor_id {
        return Err(AppError::Forbidden(
            "only the comment author can edit a comment".into(),
        ));
    }
    state
        .db
        .update_comment(tenant_id, task_id, comment_id, body)
        .await
}

pub async fn delete_comment(
    state: &AppState,
    tenant_id: Uuid,
    task_id: Uuid,
    comment_id: Uuid,
    actor_id: Uuid,
    role: MembershipRole,
) -> AppResult<()> {
    task_service::get_task(state, tenant_id, task_id).await?;
    let comment = state.db.get_comment(tenant_id, task_id, comment_id).await?;
    let is_author = comment.author_id == actor_id;
    let is_admin = matches!(role, MembershipRole::Owner | MembershipRole::Admin);
    if !is_author && !is_admin {
        return Err(AppError::Forbidden(
            "only the comment author or an owner/admin can delete a comment".into(),
        ));
    }
    state
        .db
        .delete_comment(tenant_id, task_id, comment_id, actor_id)
        .await
}
