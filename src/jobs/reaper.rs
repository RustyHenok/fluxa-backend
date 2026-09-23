use tokio::sync::watch;
use tracing::{info, warn};

use crate::error::AppResult;
use crate::services::jobs as jobs_service;
use crate::state::AppState;

pub(super) async fn reap_stale_jobs_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.worker_dispatch_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("stale job reaper shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                if let Err(error) = jobs_service::reap_stale_jobs(&state).await {
                    warn!("failed to reap stale jobs: {error}");
                }
            }
        }
    }
}
