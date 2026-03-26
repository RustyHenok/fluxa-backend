use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::error::AppResult;
use crate::services::jobs as jobs_service;
use crate::state::AppState;

pub async fn run_worker(state: AppState, shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let dispatch_state = state.clone();
    let scheduler_state = state.clone();
    let processor_state = state.clone();

    let dispatch_shutdown = shutdown.clone();
    let scheduler_shutdown = shutdown.clone();
    let processor_shutdown = shutdown.clone();

    tokio::try_join!(
        dispatch_ready_jobs_loop(dispatch_state, dispatch_shutdown),
        schedule_due_reminders_loop(scheduler_state, scheduler_shutdown),
        process_jobs_loop(processor_state, processor_shutdown),
    )?;

    Ok(())
}

async fn dispatch_ready_jobs_loop(
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

async fn schedule_due_reminders_loop(
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

async fn process_jobs_loop(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
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
