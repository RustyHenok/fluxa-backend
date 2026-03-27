use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::Response;
use metrics::counter;

use crate::error::AppResult;
use crate::state::AppState;

use super::AuthenticatedUser;
use super::helpers::{
    attach_rate_limit_headers, bearer_token, client_identifier, parse_uuid, rate_limit_response,
};

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
    counter!("http_requests_total", "route" => "auth").increment(1);
    Ok(response)
}

pub(super) async fn protected_middleware(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> AppResult<Response> {
    let token = bearer_token(request.headers())?;
    let claims = state.auth.decode_access_token(token)?;
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
    counter!("http_requests_total", "route" => "protected").increment(1);
    Ok(response)
}
