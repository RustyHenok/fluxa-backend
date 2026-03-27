use tokio::sync::watch;

use crate::error::AppResult;
use crate::state::AppState;

use super::dispatch::dispatch_ready_jobs_loop;
use super::processor::process_jobs_loop;
use super::scheduler::schedule_due_reminders_loop;

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
