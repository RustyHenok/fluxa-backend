pub mod auth;
pub mod cache;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod grpc;
pub mod http;
pub mod jobs;
pub mod pagination;
pub mod state;

use std::sync::Arc;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{Cli, ServiceMode};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub async fn run(cli: Cli) -> AppResult<()> {
    init_tracing();

    let config = Arc::new(cli.validate()?);
    let metrics = install_metrics()?;
    let db = db::Database::connect(&config).await?;
    db.migrate().await?;
    let cache = cache::CacheStore::new(config.redis_url.clone(), config.clone())?;
    let auth = auth::AuthService::new(config.clone())?;
    let state = AppState::new(config.clone(), db, cache, auth, metrics);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks = Vec::<JoinHandle<AppResult<()>>>::new();

    if matches!(config.mode, ServiceMode::Api | ServiceMode::All) {
        let http_state = state.clone();
        let grpc_state = state.clone();
        let http_rx = shutdown_rx.clone();
        let grpc_rx = shutdown_rx.clone();

        tasks.push(tokio::spawn(async move {
            http::serve(http_state, http_rx).await
        }));
        tasks.push(tokio::spawn(async move {
            grpc::serve(grpc_state, grpc_rx).await
        }));
    }

    if matches!(config.mode, ServiceMode::Worker | ServiceMode::All) {
        let worker_state = state.clone();
        let worker_rx = shutdown_rx.clone();
        tasks.push(tokio::spawn(async move {
            jobs::run_worker(worker_state, worker_rx).await
        }));
    }

    tokio::select! {
        result = wait_for_first_task(&mut tasks) => {
            if let Err(error) = shutdown_tx.send(true) {
                tracing::warn!("failed to notify shutdown: {error}");
            }
            result?;
        }
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(AppError::from)?;
            info!("shutdown signal received");
            if let Err(error) = shutdown_tx.send(true) {
                tracing::warn!("failed to notify shutdown: {error}");
            }
        }
    }

    for task in tasks {
        match task.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(AppError::internal(format!("task join error: {error}"))),
        }
    }

    Ok(())
}

async fn wait_for_first_task(tasks: &mut [JoinHandle<AppResult<()>>]) -> AppResult<()> {
    if tasks.is_empty() {
        return Ok(());
    }

    loop {
        for task in tasks.iter_mut() {
            if task.is_finished() {
                return match task.await {
                    Ok(result) => result,
                    Err(error) => Err(AppError::internal(format!("task join error: {error}"))),
                };
            }
        }

        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));

    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .json()
        .try_init();
}

fn install_metrics() -> AppResult<PrometheusHandle> {
    PrometheusBuilder::new()
        .install_recorder()
        .map_err(|error| AppError::internal(format!("failed to install metrics recorder: {error}")))
}

pub async fn bind_listener(addr: std::net::SocketAddr) -> AppResult<TcpListener> {
    TcpListener::bind(addr)
        .await
        .map_err(|error| AppError::internal(format!("failed to bind {addr}: {error}")))
}
