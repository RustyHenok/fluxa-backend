use std::sync::Arc;

use axum::extract::FromRef;
use metrics_exporter_prometheus::PrometheusHandle;

use crate::auth::AuthService;
use crate::cache::CacheStore;
use crate::config::SharedConfig;
use crate::db::Database;

#[derive(Clone, FromRef)]
pub struct AppState {
    pub config: SharedConfig,
    pub db: Database,
    pub cache: CacheStore,
    pub auth: AuthService,
    pub metrics: MetricsHandle,
}

#[derive(Clone)]
pub struct MetricsHandle {
    handle: Arc<PrometheusHandle>,
}

impl MetricsHandle {
    pub fn new(handle: PrometheusHandle) -> Self {
        Self {
            handle: Arc::new(handle),
        }
    }

    pub fn render(&self) -> String {
        self.handle.render()
    }
}

impl AppState {
    pub fn new(
        config: SharedConfig,
        db: Database,
        cache: CacheStore,
        auth: AuthService,
        metrics: PrometheusHandle,
    ) -> Self {
        Self {
            config,
            db,
            cache,
            auth,
            metrics: MetricsHandle::new(metrics),
        }
    }
}
