use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tracing::info;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::proto::job_admin_server::JobAdminServer;
use super::proto::task_read_server::TaskReadServer;
use super::services::{JobAdminService, TaskReadService};

pub async fn serve(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let listener = crate::bind_listener(state.config.grpc_addr).await?;
    let incoming = TcpListenerStream::new(listener);
    info!("grpc server listening on {}", state.config.grpc_addr);

    Server::builder()
        .add_service(JobAdminServer::new(JobAdminService::new(state.clone())))
        .add_service(TaskReadServer::new(TaskReadService::new(state)))
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|error| AppError::internal(format!("grpc server failed: {error}")))
}
