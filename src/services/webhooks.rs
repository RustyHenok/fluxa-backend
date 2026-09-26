use tracing::warn;
use uuid::Uuid;

use crate::domain::{
    WebhookDeliveryRecord, WebhookRecord, validate_webhook_events, validate_webhook_url,
};
use crate::error::{AppError, AppResult};
use crate::pagination::AuditCursor;
use crate::state::AppState;
use crate::tokens::generate_token;

pub struct WebhookDeliveryPage {
    pub deliveries: Vec<WebhookDeliveryRecord>,
    pub next_cursor: Option<AuditCursor>,
}

pub async fn list_webhooks(state: &AppState, tenant_id: Uuid) -> AppResult<Vec<WebhookRecord>> {
    state.db.list_webhooks(tenant_id).await
}

pub async fn get_webhook(
    state: &AppState,
    tenant_id: Uuid,
    webhook_id: Uuid,
) -> AppResult<WebhookRecord> {
    state.db.get_webhook(tenant_id, webhook_id).await
}

/// Registers a webhook and returns the record together with its signing
/// secret. The secret is only exposed at creation time.
pub async fn create_webhook(
    state: &AppState,
    tenant_id: Uuid,
    actor_id: Uuid,
    url: &str,
    events: Vec<String>,
) -> AppResult<WebhookRecord> {
    let url = validate_webhook_url(url, state.config.webhook_allow_private_urls)?;
    let events = validate_webhook_events(events)?;

    // The per-tenant cap is enforced atomically inside `Database::create_webhook`.
    let secret = generate_token();
    state
        .db
        .create_webhook(tenant_id, actor_id, &url, &secret, &events)
        .await
}

pub async fn update_webhook(
    state: &AppState,
    tenant_id: Uuid,
    webhook_id: Uuid,
    url: Option<String>,
    events: Option<Vec<String>>,
    is_active: Option<bool>,
) -> AppResult<WebhookRecord> {
    if url.is_none() && events.is_none() && is_active.is_none() {
        return Err(AppError::Validation(
            "at least one webhook field must be provided".into(),
        ));
    }

    let url = url
        .map(|value| validate_webhook_url(&value, state.config.webhook_allow_private_urls))
        .transpose()?;
    let events = events.map(validate_webhook_events).transpose()?;

    state
        .db
        .update_webhook(
            tenant_id,
            webhook_id,
            url.as_deref(),
            events.as_deref(),
            is_active,
        )
        .await
}

pub async fn delete_webhook(state: &AppState, tenant_id: Uuid, webhook_id: Uuid) -> AppResult<()> {
    state.db.delete_webhook(tenant_id, webhook_id).await
}

pub async fn list_webhook_deliveries(
    state: &AppState,
    tenant_id: Uuid,
    webhook_id: Uuid,
    cursor: Option<&AuditCursor>,
    limit: usize,
) -> AppResult<WebhookDeliveryPage> {
    state.db.get_webhook(tenant_id, webhook_id).await?;
    let page = state
        .db
        .list_webhook_deliveries(tenant_id, webhook_id, cursor, limit)
        .await?;
    Ok(WebhookDeliveryPage {
        deliveries: page.deliveries,
        next_cursor: page.next_cursor,
    })
}

/// Queues an event for all subscribed webhooks. Failures are logged and
/// swallowed so webhook fan-out never breaks the primary operation.
pub async fn emit_task_event(
    state: &AppState,
    tenant_id: Uuid,
    event_type: &str,
    payload: &serde_json::Value,
) {
    if let Err(error) = state
        .db
        .enqueue_webhook_deliveries(tenant_id, event_type, payload)
        .await
    {
        warn!(
            %tenant_id,
            event_type,
            "failed to enqueue webhook deliveries: {error}"
        );
    }
}
