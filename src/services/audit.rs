use serde_json::Value;
use tracing::warn;
use uuid::Uuid;

use crate::domain::{AuditEventResponse, PaginatedAuditEvents};
use crate::error::AppResult;
use crate::pagination::AuditCursor;
use crate::state::AppState;

/// Records an audit event without failing the caller: the business operation
/// has already succeeded, so audit write failures are logged and swallowed.
pub async fn record_event(
    state: &AppState,
    tenant_id: Option<Uuid>,
    actor_user_id: Option<Uuid>,
    subject_type: &str,
    subject_id: Option<Uuid>,
    event_type: &str,
    payload: Value,
) {
    if let Err(error) = state
        .db
        .record_audit_event(
            tenant_id,
            actor_user_id,
            subject_type,
            subject_id,
            event_type,
            payload,
        )
        .await
    {
        warn!(event_type, "failed to record audit event: {error}");
    }
}

pub async fn list_events(
    state: &AppState,
    tenant_id: Uuid,
    cursor: Option<&AuditCursor>,
    limit: usize,
) -> AppResult<PaginatedAuditEvents> {
    let (entries, next_cursor) = state.db.list_audit_events(tenant_id, cursor, limit).await?;

    let next_cursor = next_cursor.map(|cursor| cursor.encode()).transpose()?;

    Ok(PaginatedAuditEvents {
        data: entries.iter().map(AuditEventResponse::from).collect(),
        next_cursor,
    })
}
