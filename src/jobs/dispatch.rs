use tokio::sync::watch;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::services::jobs as jobs_service;
use crate::state::AppState;

pub(super) async fn dispatch_ready_jobs_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.worker_dispatch_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("job dispatcher shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                if let Err(error) = jobs_service::dispatch_ready_jobs(&state, 100).await {
                    warn!("failed to dispatch ready jobs: {error}");
                }
            }
        }
    }
}
