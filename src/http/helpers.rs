use std::net::SocketAddr;

use axum::Json;
use axum::extract::{ConnectInfo, Request};
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde_json::Value;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

use crate::cache::IdempotencyState;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub(super) fn build_cors(state: &AppState) -> AppResult<CorsLayer> {
    let base = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    if state.config.cors_allow_origin == "*" {
        Ok(base.allow_origin(Any))
    } else {
        let origin: HeaderValue =
            state.config.cors_allow_origin.parse().map_err(|error| {
                AppError::Validation(format!("invalid CORS_ALLOW_ORIGIN: {error}"))
            })?;
        Ok(base.allow_origin(origin))
    }
}

pub(super) fn normalize_email(email: &str) -> AppResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') {
        return Err(AppError::Validation("email must be valid".into()));
    }
    Ok(email)
}

pub(super) fn validate_password(password: &str) -> AppResult<()> {
    if password.len() < 8 {
        return Err(AppError::Validation(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

pub(super) fn normalize_optional_choice(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_ascii_lowercase())
}

pub(super) fn parse_uuid(value: &str, label: &str) -> AppResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::Unauthorized(format!("invalid {label}: {error}")))
}

pub(super) fn ensure_task_write_role(role: &str) -> AppResult<()> {
    match role {
        "owner" | "admin" | "member" => Ok(()),
        _ => Err(AppError::Forbidden(
            "role is not allowed to modify tasks".into(),
        )),
    }
}

pub(super) fn ensure_admin_role(role: &str) -> AppResult<()> {
    match role {
        "owner" | "admin" => Ok(()),
        _ => Err(AppError::Forbidden(
            "owner or admin role required for this action".into(),
        )),
    }
}

pub(super) fn required_idempotency_key(headers: &HeaderMap) -> AppResult<&str> {
    headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Validation("Idempotency-Key header is required".into()))
}

pub(super) async fn replay_idempotent(
    state: &AppState,
    cache_key: &str,
) -> AppResult<Option<(StatusCode, Json<Value>)>> {
    match state.cache.idempotency_state(cache_key).await? {
        IdempotencyState::Empty | IdempotencyState::Pending => Ok(None),
        IdempotencyState::Ready(response) => {
            let status = StatusCode::from_u16(response.status).map_err(|error| {
                AppError::internal(format!("invalid stored status code: {error}"))
            })?;
            Ok(Some((status, Json(response.body))))
        }
    }
}

pub(super) fn bearer_token(headers: &HeaderMap) -> AppResult<&str> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("missing authorization header".into()))?;
    value
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::Unauthorized("expected bearer token".into()))
}

pub(super) fn client_identifier(request: &Request) -> String {
    if let Some(value) = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|value| value.to_str().ok())
    {
        if let Some(first) = value.split(',').next() {
            return first.trim().to_string();
        }
    }

    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0.ip().to_string())
        .unwrap_or_else(|| "unknown".into())
}

pub(super) fn attach_rate_limit_headers(
    response: &mut Response,
    decision: &crate::cache::RateLimitDecision,
) -> AppResult<()> {
    let remaining = HeaderValue::from_str(&decision.remaining_tokens.to_string())
        .map_err(|error| AppError::internal(format!("invalid rate limit header: {error}")))?;
    let retry = HeaderValue::from_str(&decision.retry_after.as_secs().to_string())
        .map_err(|error| AppError::internal(format!("invalid retry-after header: {error}")))?;
    response
        .headers_mut()
        .insert("x-ratelimit-remaining", remaining);
    response.headers_mut().insert("retry-after", retry);
    Ok(())
}

pub(super) fn rate_limit_response(decision: crate::cache::RateLimitDecision) -> Response {
    let mut response = AppError::RateLimited("rate limit exceeded".into()).into_response();
    let _ = attach_rate_limit_headers(&mut response, &decision);
    response
}

pub(super) fn parse_optional_datetime(
    value: Option<String>,
    field: &str,
) -> AppResult<Option<DateTime<Utc>>> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value.trim())
                .map(|value| value.with_timezone(&Utc))
                .map_err(|error| {
                    AppError::Validation(format!("{field} must be an RFC3339 timestamp: {error}"))
                })
        })
        .transpose()
}
