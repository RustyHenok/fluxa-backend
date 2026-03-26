use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, ValueEnum};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Copy, Eq, PartialEq, ValueEnum)]
pub enum ServiceMode {
    Api,
    Worker,
    All,
}

#[derive(Debug, Clone, Parser)]
#[command(
    author,
    version,
    about = "Enterprise-grade multi-tenant task platform API"
)]
pub struct Cli {
    #[arg(long, env = "APP_NAME", default_value = "fluxa-backend")]
    pub app_name: String,
    #[arg(long, env = "APP_MODE", value_enum, default_value = "all")]
    pub mode: ServiceMode,
    #[arg(long, env = "HTTP_ADDR", default_value = "0.0.0.0:8080")]
    pub http_addr: SocketAddr,
    #[arg(long, env = "GRPC_ADDR", default_value = "0.0.0.0:50051")]
    pub grpc_addr: SocketAddr,
    #[arg(long, env = "DATABASE_URL")]
    pub database_url: String,
    #[arg(long, env = "REDIS_URL", default_value = "redis://127.0.0.1/")]
    pub redis_url: String,
    #[arg(long, env = "JWT_SECRET")]
    pub jwt_secret: String,
    #[arg(long, env = "ACCESS_TOKEN_MINUTES", default_value_t = 15)]
    pub access_token_minutes: i64,
    #[arg(long, env = "REFRESH_TOKEN_DAYS", default_value_t = 30)]
    pub refresh_token_days: i64,
    #[arg(long, env = "CACHE_TTL_SECONDS", default_value_t = 60)]
    pub cache_ttl_seconds: u64,
    #[arg(long, env = "IDEMPOTENCY_TTL_SECONDS", default_value_t = 3600)]
    pub idempotency_ttl_seconds: u64,
    #[arg(long, env = "AUTH_RATE_LIMIT_CAPACITY", default_value_t = 10)]
    pub auth_rate_limit_capacity: u64,
    #[arg(long, env = "AUTH_RATE_LIMIT_REFILL_PER_SEC", default_value_t = 2)]
    pub auth_rate_limit_refill_per_sec: u64,
    #[arg(long, env = "APP_RATE_LIMIT_CAPACITY", default_value_t = 120)]
    pub app_rate_limit_capacity: u64,
    #[arg(long, env = "APP_RATE_LIMIT_REFILL_PER_SEC", default_value_t = 30)]
    pub app_rate_limit_refill_per_sec: u64,
    #[arg(long, env = "JOB_QUEUE_NAME", default_value = "jobs:queue:default")]
    pub job_queue_name: String,
    #[arg(long, env = "WORKER_DISPATCH_INTERVAL_MS", default_value_t = 3_000)]
    pub worker_dispatch_interval_ms: u64,
    #[arg(long, env = "WORKER_SCHEDULER_INTERVAL_MS", default_value_t = 60_000)]
    pub worker_scheduler_interval_ms: u64,
    #[arg(long, env = "JOB_QUEUE_BLOCK_TIMEOUT_SECONDS", default_value_t = 5)]
    pub job_queue_block_timeout_seconds: usize,
    #[arg(long, env = "MAX_JOB_ATTEMPTS", default_value_t = 5)]
    pub max_job_attempts: i32,
    #[arg(long, env = "DATABASE_MAX_CONNECTIONS", default_value_t = 10)]
    pub database_max_connections: u32,
    #[arg(long, env = "STARTUP_MAX_RETRIES", default_value_t = 20)]
    pub startup_max_retries: u32,
    #[arg(long, env = "STARTUP_RETRY_DELAY_MS", default_value_t = 1_500)]
    pub startup_retry_delay_ms: u64,
    #[arg(long, env = "CORS_ALLOW_ORIGIN", default_value = "*")]
    pub cors_allow_origin: String,
}

impl Cli {
    pub fn validate(self) -> AppResult<Self> {
        if self.jwt_secret.len() < 32 {
            return Err(AppError::Validation(
                "JWT_SECRET must be at least 32 characters long".into(),
            ));
        }

        if self.access_token_minutes <= 0 || self.refresh_token_days <= 0 {
            return Err(AppError::Validation(
                "token lifetimes must be positive".into(),
            ));
        }

        if self.auth_rate_limit_capacity == 0
            || self.auth_rate_limit_refill_per_sec == 0
            || self.app_rate_limit_capacity == 0
            || self.app_rate_limit_refill_per_sec == 0
        {
            return Err(AppError::Validation(
                "rate limit capacity and refill must be positive".into(),
            ));
        }

        if self.max_job_attempts <= 0 {
            return Err(AppError::Validation(
                "MAX_JOB_ATTEMPTS must be positive".into(),
            ));
        }

        if self.startup_max_retries == 0 || self.startup_retry_delay_ms == 0 {
            return Err(AppError::Validation(
                "startup retry settings must be positive".into(),
            ));
        }

        Ok(self)
    }

    pub fn access_token_ttl(&self) -> Duration {
        Duration::from_secs((self.access_token_minutes * 60) as u64)
    }

    pub fn refresh_token_ttl(&self) -> Duration {
        Duration::from_secs((self.refresh_token_days * 24 * 60 * 60) as u64)
    }

    pub fn cache_ttl(&self) -> Duration {
        Duration::from_secs(self.cache_ttl_seconds)
    }

    pub fn idempotency_ttl(&self) -> Duration {
        Duration::from_secs(self.idempotency_ttl_seconds)
    }

    pub fn worker_dispatch_interval(&self) -> Duration {
        Duration::from_millis(self.worker_dispatch_interval_ms)
    }

    pub fn worker_scheduler_interval(&self) -> Duration {
        Duration::from_millis(self.worker_scheduler_interval_ms)
    }

    pub fn startup_retry_delay(&self) -> Duration {
        Duration::from_millis(self.startup_retry_delay_ms)
    }
}

pub type SharedConfig = Arc<Cli>;
