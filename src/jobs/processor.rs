use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::error::AppResult;
use crate::services::jobs as jobs_service;
use crate::state::AppState;

pub(super) async fn process_jobs_loop(
    state: AppState,
    mut shutdown: watch::Receiver<bool>,
) -> AppResult<()> {
    loop {
        tokio::select! {
            _ = shutdown.changed() => {
                info!("job processor shutting down");
                return Ok(());
            }
            result = state.cache.dequeue_job(state.config.job_queue_block_timeout_seconds) => {
                match result {
                    Ok(Some(job_id)) => {
                        if let Err(error) = jobs_service::process_job(&state, job_id).await {
                            error!("job processing failed for {job_id}: {error}");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => warn!("failed to dequeue job: {error}"),
                }
            }
        }
    }
}
