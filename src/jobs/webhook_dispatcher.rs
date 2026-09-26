use std::time::Duration;

use metrics::counter;
use serde_json::json;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::domain::webhook_signature;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const WEBHOOK_BATCH_SIZE: i64 = 50;
const WEBHOOK_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) async fn dispatch_webhooks_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let client = reqwest::Client::builder()
        .timeout(WEBHOOK_REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| AppError::internal(format!("failed to build webhook client: {error}")))?;
    let mut interval = tokio::time::interval(state.config.webhook_dispatch_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("webhook dispatcher shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                if let Err(error) = dispatch_pending_webhooks(&state, &client).await {
                    warn!("failed to dispatch webhooks: {error}");
                }
            }
        }
    }
}

async fn dispatch_pending_webhooks(state: &AppState, client: &reqwest::Client) -> AppResult<()> {
    let deliveries = state
        .db
        .claim_pending_webhook_deliveries(WEBHOOK_BATCH_SIZE)
        .await?;

    for delivery in &deliveries {
        let outcome = send_delivery(state, client, delivery).await;
        match outcome {
            Ok(()) => {
                state
                    .db
                    .mark_webhook_delivery_delivered(delivery.id)
                    .await?;
                counter!("webhooks_delivered_total").increment(1);
            }
            Err(error) => {
                warn!(
                    delivery_id = %delivery.id,
                    webhook_id = %delivery.webhook_id,
                    event_type = %delivery.event_type,
                    "webhook delivery failed: {error}"
                );
                state
                    .db
                    .fail_webhook_delivery(delivery, &error.to_string())
                    .await?;
                counter!("webhooks_failed_total").increment(1);
            }
        }
    }

    Ok(())
}

async fn send_delivery(
    state: &AppState,
    client: &reqwest::Client,
    delivery: &crate::domain::WebhookDeliveryRecord,
) -> AppResult<()> {
    let webhook = state
        .db
        .get_webhook(delivery.tenant_id, delivery.webhook_id)
        .await?;
    if !webhook.is_active {
        return Err(AppError::Validation("webhook is disabled".into()));
    }

    let body = serde_json::to_vec(&json!({
        "id": delivery.id,
        "event": delivery.event_type,
        "created_at": delivery.created_at,
        "data": delivery.payload,
    }))
    .map_err(|error| AppError::internal(format!("failed to serialize delivery: {error}")))?;
    let signature = webhook_signature(&webhook.secret, &body);

    let response = client
        .post(&webhook.url)
        .header("content-type", "application/json")
        .header("x-fluxa-event", &delivery.event_type)
        .header("x-fluxa-delivery", delivery.id.to_string())
        .header("x-fluxa-signature", signature)
        .body(body)
        .send()
        .await
        .map_err(|error| AppError::internal(format!("webhook request failed: {error}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(AppError::internal(format!(
            "webhook endpoint responded with status {status}"
        )));
    }

    Ok(())
}
