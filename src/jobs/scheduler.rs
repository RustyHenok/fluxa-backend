use tokio::sync::watch;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::services::jobs as jobs_service;
use crate::state::AppState;

pub(super) async fn schedule_due_reminders_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.worker_scheduler_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("due reminder scheduler shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                match jobs_service::enqueue_due_reminder_sweep(&state, None).await {
                    Ok(Some(job)) => {
                        info!("scheduled due reminder job {}", job.id);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        warn!("failed to schedule due reminder job: {error}");
                    }
                }
            }
        }
    }
}
