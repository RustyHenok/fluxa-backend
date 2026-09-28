use metrics::gauge;
use tokio::sync::watch;
use tracing::{debug, info};

use crate::error::AppResult;
use crate::state::AppState;

/// Periodically samples DB pool and job queue depth gauges so operators can
/// watch saturation without instrumenting every call site.
pub async fn run_sampler(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let mut interval = tokio::time::interval(state.config.sampler_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("metrics sampler shutting down");
                return Ok(());
            }
            _ = interval.tick() => {
                sample(&state).await;
            }
        }
    }
}

async fn sample(state: &AppState) {
    let (size, idle) = state.db.pool_stats();
    gauge!("db_pool_connections", "state" => "open").set(size as f64);
    gauge!("db_pool_connections", "state" => "idle").set(idle as f64);

    match state.db.count_queued_jobs().await {
        Ok(count) => gauge!("jobs_queued").set(count as f64),
        Err(error) => debug!("failed to sample queued job count: {error}"),
    }

    match state.cache.job_queue_depth().await {
        Ok(depth) => gauge!("job_queue_depth").set(depth as f64),
        Err(error) => debug!("failed to sample job queue depth: {error}"),
    }
}
