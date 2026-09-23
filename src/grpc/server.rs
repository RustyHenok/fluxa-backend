use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::Server;
use tracing::info;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

use super::auth::SharedTokenInterceptor;
use super::proto::job_admin_server::JobAdminServer;
use super::proto::task_read_server::TaskReadServer;
use super::services::{JobAdminService, TaskReadService};

pub async fn serve(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let listener = crate::bind_listener(state.config.grpc_addr).await?;
    let incoming = TcpListenerStream::new(listener);
    info!("grpc server listening on {}", state.config.grpc_addr);

    let interceptor = SharedTokenInterceptor::new(state.config.grpc_auth_token.clone());

    Server::builder()
        .add_service(JobAdminServer::with_interceptor(
            JobAdminService::new(state.clone()),
            interceptor.clone(),
        ))
        .add_service(TaskReadServer::with_interceptor(
            TaskReadService::new(state),
            interceptor,
        ))
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|error| AppError::internal(format!("grpc server failed: {error}")))
}
