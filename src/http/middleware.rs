use std::time::Instant;

use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::Response;
use metrics::{counter, histogram};
use opentelemetry::trace::TraceContextExt;
use tracing_opentelemetry::OpenTelemetrySpanExt;

use crate::error::AppResult;
use crate::state::AppState;

use super::AuthenticatedUser;
use super::helpers::{
    attach_rate_limit_headers, bearer_token, client_identifier, parse_uuid, rate_limit_response,
};

struct HeaderContextExtractor<'a>(&'a axum::http::HeaderMap);

impl opentelemetry::propagation::Extractor for HeaderContextExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.0.keys().map(|key| key.as_str()).collect()
    }
}

/// Adopts an inbound W3C `traceparent`/`tracestate` context as the parent of
/// the request span so distributed traces continue across services. This is a
/// no-op unless OTLP trace export is configured.
pub(super) async fn propagate_trace_context(request: Request, next: Next) -> Response {
    let parent = opentelemetry::global::get_text_map_propagator(|propagator| {
        propagator.extract(&HeaderContextExtractor(request.headers()))
    });
    if parent.span().span_context().is_remote() {
        tracing::Span::current().set_parent(parent);
    }
    next.run(request).await
}

/// Records a labeled request counter and duration histogram for every HTTP
/// request, keyed by method and the matched route template.
pub(super) async fn track_metrics_middleware(request: Request, next: Next) -> Response {
    let method = request.method().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str().to_owned())
        .unwrap_or_else(|| "unmatched".to_owned());

    let started_at = Instant::now();
    let response = next.run(request).await;
    let elapsed = started_at.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    counter!(
        "http_requests_total",
        "method" => method.clone(),
        "route" => route.clone(),
        "status" => status
    )
    .increment(1);
    histogram!(
        "http_request_duration_seconds",
        "method" => method,
        "route" => route
    )
    .record(elapsed);

    response
}

pub(super) async fn auth_rate_limit_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> AppResult<Response> {
    let key = format!(
        "ratelimit:auth:{}:{}",
        request.uri().path(),
        client_identifier(&request)
    );
    let decision = state
        .cache
        .rate_limit(
            &key,
            state.config.auth_rate_limit_capacity,
            state.config.auth_rate_limit_refill_per_sec,
            1,
        )
        .await?;

    if !decision.allowed {
        return Ok(rate_limit_response(decision));
    }

    let mut response = next.run(request).await;
    attach_rate_limit_headers(&mut response, &decision)?;
    Ok(response)
}

pub(super) async fn protected_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer_token(request.headers())?;
    let claims = state.auth.decode_access_token(token)?;
    if state.cache.is_access_token_denied(&claims.jti).await? {
        return Err(crate::error::AppError::Unauthorized(
            "access token has been revoked".into(),
        ));
    }
    let user_id = parse_uuid(&claims.sub, "user id")?;
    let tenant_id = parse_uuid(&claims.tenant_id, "tenant id")?;
    let membership = state
        .db
        .get_membership(user_id, tenant_id)
        .await?
        .ok_or_else(|| crate::error::AppError::Unauthorized("membership not found".into()))?;

    let auth_user = AuthenticatedUser {
        user_id,
        tenant_id,
        role: membership.parsed_role()?,
    };
    request.extensions_mut().insert(auth_user.clone());

    let key = format!(
        "ratelimit:app:{}:{}:{}",
        tenant_id,
        user_id,
        client_identifier(&request)
    );
    let decision = state
        .cache
        .rate_limit(
            &key,
            state.config.app_rate_limit_capacity,
            state.config.app_rate_limit_refill_per_sec,
            1,
        )
        .await?;

    if !decision.allowed {
        return Ok(rate_limit_response(decision));
    }

    let mut response = next.run(request).await;
    attach_rate_limit_headers(&mut response, &decision)?;
    Ok(response)
}
