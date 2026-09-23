use metrics::counter;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::notify::{AnyMailer, Mailer, render_notification};
use crate::state::AppState;

const NOTIFICATION_BATCH_SIZE: i64 = 50;

pub(super) async fn deliver_notifications_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mailer = AnyMailer::from_config(&state.config)?;
    let mut interval = tokio::time::interval(state.config.notify_dispatch_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("notification worker shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                if let Err(error) = deliver_pending_notifications(&state, &mailer).await {
                    warn!("failed to deliver notifications: {error}");
                }
            }
        }
    }
}

async fn deliver_pending_notifications(state: &AppState, mailer: &AnyMailer) -> AppResult<()> {
    let notifications = state
        .db
        .claim_pending_notifications(NOTIFICATION_BATCH_SIZE)
        .await?;

    for notification in &notifications {
        let outcome = match render_notification(notification) {
            Ok(message) => mailer.send(&message).await,
            Err(error) => Err(error),
        };

        match outcome {
            Ok(()) => {
                state.db.mark_notification_sent(notification.id).await?;
                counter!("notifications_sent_total").increment(1);
            }
            Err(error) => {
                warn!(
                    notification_id = %notification.id,
                    kind = %notification.kind,
                    "notification delivery failed: {error}"
                );
                state
                    .db
                    .fail_notification(notification, &error.to_string())
                    .await?;
                counter!("notifications_failed_total").increment(1);
            }
        }
    }

    Ok(())
}
