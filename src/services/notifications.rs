//! In-app notification feed: exposes the user's task-activity notifications
//! with read tracking. Only the kinds in [`FEED_KINDS`] are ever listed or
//! addressable so token-bearing account mails (verification, password reset,
//! invitations) never leak through this surface.

use uuid::Uuid;

use crate::domain::{NotificationFeedItemResponse, NotificationFeedResponse};
use crate::error::{AppError, AppResult};
use crate::notify::{KIND_TASK_COMMENTED, KIND_TASK_DUE_SOON, KIND_TASK_OVERDUE};
use crate::pagination::AuditCursor;
use crate::state::AppState;

/// Notification kinds visible in the in-app feed.
pub const FEED_KINDS: &[&str] = &[KIND_TASK_DUE_SOON, KIND_TASK_OVERDUE, KIND_TASK_COMMENTED];

/// Lists the user's feed for the active tenant, newest first, along with the
/// unread badge count.
pub async fn list_feed(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    unread_only: bool,
    cursor: Option<&AuditCursor>,
    limit: usize,
) -> AppResult<NotificationFeedResponse> {
    let (entries, next_cursor) = state
        .db
        .list_notification_feed(tenant_id, user_id, FEED_KINDS, unread_only, cursor, limit)
        .await?;
    let unread_count = state
        .db
        .count_unread_notifications(tenant_id, user_id, FEED_KINDS)
        .await?;

    let next_cursor = next_cursor.map(|cursor| cursor.encode()).transpose()?;

    Ok(NotificationFeedResponse {
        data: entries
            .iter()
            .map(NotificationFeedItemResponse::from)
            .collect(),
        next_cursor,
        unread_count,
    })
}

/// Marks one feed notification as read. Errors with `NotFound` when the id is
/// not one of the caller's feed entries.
pub async fn mark_read(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    notification_id: Uuid,
) -> AppResult<()> {
    let updated = state
        .db
        .mark_notification_read(tenant_id, user_id, notification_id, FEED_KINDS)
        .await?;
    if !updated {
        return Err(AppError::NotFound("notification was not found".into()));
    }
    Ok(())
}

/// Marks every unread feed notification as read, returning how many changed.
pub async fn mark_all_read(state: &AppState, tenant_id: Uuid, user_id: Uuid) -> AppResult<u64> {
    state
        .db
        .mark_all_notifications_read(tenant_id, user_id, FEED_KINDS)
        .await
}
