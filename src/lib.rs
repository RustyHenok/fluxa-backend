#![recursion_limit = "512"]

pub mod auth;
pub mod cache;
pub mod config;
pub mod db;
pub mod domain;
pub mod error;
pub mod grpc;
pub mod http;
pub mod jobs;
pub mod openapi;
pub mod pagination;
pub mod sampler;
pub mod services;
pub mod state;
pub mod storage;
pub mod tokens;

pub mod notify;

use std::sync::Arc;

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

use crate::config::{Cli, ServiceMode};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub async fn run(cli: Cli) -> AppResult<()> {
    let tracer_provider = init_tracing(&cli);

    let config = Arc::new(cli.validate()?);
    let metrics = install_metrics()?;
    let db = connect_database_with_retry(config.clone()).await?;
    db.migrate().await?;
    let cache = cache::CacheStore::new(config.redis_url.clone(), config.clone())?;
    wait_for_cache_with_retry(&cache, config.clone()).await?;
    let auth = auth::AuthService::new(config.clone())?;
    let state = AppState::new(config.clone(), db, cache, auth, metrics);

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let mut tasks = JoinSet::<AppResult<()>>::new();

    if matches!(config.mode, ServiceMode::Api | ServiceMode::All) {
        let http_state = state.clone();
        let grpc_state = state.clone();
        let http_rx = shutdown_rx.clone();
        let grpc_rx = shutdown_rx.clone();

        tasks.spawn(async move { http::serve(http_state, http_rx).await });
        tasks.spawn(async move { grpc::serve(grpc_state, grpc_rx).await });
    }

    if matches!(config.mode, ServiceMode::Worker | ServiceMode::All) {
        let worker_state = state.clone();
        let worker_rx = shutdown_rx.clone();
        tasks.spawn(async move { jobs::run_worker(worker_state, worker_rx).await });
    }

    let sampler_state = state.clone();
    let sampler_rx = shutdown_rx.clone();
    tasks.spawn(async move { sampler::run_sampler(sampler_state, sampler_rx).await });

    tokio::select! {
        joined = tasks.join_next() => {
            if let Err(error) = shutdown_tx.send(true) {
                tracing::warn!("failed to notify shutdown: {error}");
            }
            match joined {
                Some(Ok(result)) => result?,
                Some(Err(error)) => {
                    return Err(AppError::internal(format!("task join error: {error}")));
                }
                None => {}
            }
        }
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(AppError::from)?;
            info!("shutdown signal received");
            if let Err(error) = shutdown_tx.send(true) {
                tracing::warn!("failed to notify shutdown: {error}");
            }
        }
    }

    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(Ok(())) => {}
            Ok(Err(error)) => return Err(error),
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(AppError::internal(format!("task join error: {error}"))),
        }
    }

    shutdown_tracing(tracer_provider).await;

    Ok(())
}

fn init_tracing(cli: &Cli) -> Option<opentelemetry_sdk::trace::TracerProvider> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,sqlx=warn,tower_http=info"));
    let fmt_layer = tracing_subscriber::fmt::layer().with_target(true).json();

    let (otel_layer, provider) = match cli
        .otlp_endpoint()
        .map(|endpoint| build_tracer_provider(endpoint, cli.otel_service_name.clone()))
    {
        Some(Ok(provider)) => {
            opentelemetry::global::set_text_map_propagator(
                opentelemetry_sdk::propagation::TraceContextPropagator::new(),
            );
            opentelemetry::global::set_tracer_provider(provider.clone());
            let tracer = {
                use opentelemetry::trace::TracerProvider as _;
                provider.tracer("fluxa-backend")
            };
            (
                Some(tracing_opentelemetry::layer().with_tracer(tracer)),
                Some(provider),
            )
        }
        Some(Err(error)) => {
            eprintln!("failed to initialise OTLP trace export, continuing without it: {error}");
            (None, None)
        }
        None => (None, None),
    };

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .with(otel_layer)
        .try_init();

    if provider.is_some() {
        info!("OTLP trace export enabled");
    }
    provider
}

fn build_tracer_provider(
    endpoint: &str,
    service_name: String,
) -> Result<opentelemetry_sdk::trace::TracerProvider, opentelemetry::trace::TraceError> {
    use opentelemetry_otlp::WithExportConfig;

    let exporter = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_endpoint(endpoint)
        .build()?;

    Ok(opentelemetry_sdk::trace::TracerProvider::builder()
        .with_batch_exporter(exporter, opentelemetry_sdk::runtime::Tokio)
        .with_resource(opentelemetry_sdk::Resource::new(vec![
            opentelemetry::KeyValue::new("service.name", service_name),
        ]))
        .build())
}

async fn shutdown_tracing(provider: Option<opentelemetry_sdk::trace::TracerProvider>) {
    let Some(provider) = provider else {
        return;
    };
    // Shut down on a blocking thread: flushing the batch processor blocks on
    // the export channel, which can deadlock inside the async runtime.
    match tokio::task::spawn_blocking(move || provider.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => warn!("failed to flush OTLP spans on shutdown: {error}"),
        Err(error) => warn!("failed to join OTLP shutdown task: {error}"),
    }
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

async fn connect_database_with_retry(config: config::SharedConfig) -> AppResult<db::Database> {
    let mut last_error = None;

    for attempt in 1..=config.startup_max_retries {
        match db::Database::connect(&config).await {
            Ok(database) => return Ok(database),
            Err(error) => {
                warn!(
                    "database connection attempt {attempt}/{} failed: {error}",
                    config.startup_max_retries
                );
                last_error = Some(error);
                tokio::time::sleep(config.startup_retry_delay()).await;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| AppError::internal("database connection failed")))
}

async fn wait_for_cache_with_retry(
    cache: &cache::CacheStore,
    config: config::SharedConfig,
) -> AppResult<()> {
    let mut last_error = None;

    for attempt in 1..=config.startup_max_retries {
        match cache.ping().await {
            Ok(()) => return Ok(()),
            Err(error) => {
                warn!(
                    "redis readiness attempt {attempt}/{} failed: {error}",
                    config.startup_max_retries
                );
                last_error = Some(error);
                tokio::time::sleep(config.startup_retry_delay()).await;
            }
        }
    }

    Err(last_error.unwrap_or_else(|| AppError::internal("redis readiness failed")))
}
