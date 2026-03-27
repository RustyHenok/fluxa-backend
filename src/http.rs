use std::net::SocketAddr;
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::middleware as axum_middleware;
use axum::routing::{get, post};
use tower_http::compression::CompressionLayer;
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use uuid::Uuid;

use crate::domain::MembershipRole;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

mod dto;
mod handlers;
mod helpers;
mod middleware;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub role: MembershipRole,
}

pub async fn serve(
    state: AppState,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> AppResult<()> {
    let router = router(state.clone())?;
    let listener = crate::bind_listener(state.config.http_addr).await?;
    info!("http server listening on {}", state.config.http_addr);

    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let _ = shutdown.changed().await;
    })
    .await
    .map_err(AppError::from)
}

fn router(state: AppState) -> AppResult<Router> {
    let auth_routes = Router::new()
        .route("/auth/register", post(handlers::register))
        .route("/auth/login", post(handlers::login))
        .route("/auth/refresh", post(handlers::refresh))
        .route("/auth/logout", post(handlers::logout))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::auth_rate_limit_middleware,
        ));

    let protected_routes = Router::new()
        .route("/auth/switch-tenant", post(handlers::switch_tenant))
        .route("/dashboard/summary", get(handlers::dashboard_summary))
        .route("/me", get(handlers::me))
        .route("/me/tenants", get(handlers::list_my_tenants))
        .route(
            "/tenants/:tenant_id/members",
            get(handlers::list_tenant_members),
        )
        .route(
            "/tasks",
            get(handlers::list_tasks).post(handlers::create_task),
        )
        .route(
            "/tasks/:task_id",
            get(handlers::get_task)
                .patch(handlers::update_task)
                .delete(handlers::delete_task),
        )
        .route("/tasks/:task_id/audit", get(handlers::list_task_audit))
        .route("/exports/tasks", post(handlers::create_export))
        .route("/jobs/:job_id", get(handlers::get_job))
        .layer(axum_middleware::from_fn_with_state(
            state.clone(),
            middleware::protected_middleware,
        ));

    let cors = helpers::build_cors(&state)?;

    Ok(Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/readyz", get(handlers::readyz))
        .route("/metrics", get(handlers::metrics))
        .nest(
            "/v1",
            Router::new().merge(auth_routes).merge(protected_routes),
        )
        .layer(TraceLayer::new_for_http())
        .layer(CompressionLayer::new())
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(30),
        ))
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(cors)
        .with_state(state))
}

#[cfg(test)]
mod tests {
    use super::helpers::{normalize_optional_choice, parse_optional_datetime};
    use crate::domain::{TASK_PRIORITY_HIGH, TASK_STATUS_OPEN};

    #[test]
    fn query_helpers_normalize_values() {
        assert_eq!(
            normalize_optional_choice(Some(" OPEN ".into())).as_deref(),
            Some(TASK_STATUS_OPEN)
        );
        assert_eq!(
            normalize_optional_choice(Some("HIGH".into())).as_deref(),
            Some(TASK_PRIORITY_HIGH)
        );
    }

    #[test]
    fn parses_rfc3339_filters() {
        let parsed = parse_optional_datetime(Some("2026-03-26T12:00:00Z".into()), "due_before")
            .expect("date should parse");
        assert!(parsed.is_some());
    }
}
