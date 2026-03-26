use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, Extension, Path, Query, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use metrics::counter;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tower_http::compression::CompressionLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use uuid::Uuid;

use crate::cache::{IdempotencyState, StoredResponse};
use crate::domain::{
    CreateTaskInput, JobResponse, MembershipRecord, TaskFilters, TaskResponse,
    TenantMembershipResponse, UpdateTaskInput, UserResponse,
};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub role: String,
}

#[derive(Debug, Deserialize)]
struct RegisterRequest {
    email: String,
    password: String,
    tenant_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct RefreshRequest {
    refresh_token: String,
    tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
struct LogoutRequest {
    refresh_token: String,
}

#[derive(Debug, Deserialize)]
struct TaskPayload {
    title: String,
    description: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    assignee_id: Option<Uuid>,
    due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Default)]
struct TaskPatchPayload {
    title: Option<String>,
    description: Option<Option<String>>,
    status: Option<String>,
    priority: Option<String>,
    assignee_id: Option<Option<Uuid>>,
    due_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
struct TaskListQuery {
    limit: Option<usize>,
    cursor: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    assignee_id: Option<Uuid>,
    due_before: Option<String>,
    due_after: Option<String>,
    updated_after: Option<String>,
    q: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
struct ExportRequest {
    status: Option<String>,
    priority: Option<String>,
    assignee_id: Option<Uuid>,
    due_before: Option<String>,
    due_after: Option<String>,
    updated_after: Option<String>,
    q: Option<String>,
}

#[derive(Debug, Serialize)]
struct AuthResponse {
    access_token: String,
    refresh_token: String,
    expires_in_seconds: u64,
    user: UserResponse,
    active_tenant: TenantMembershipResponse,
}

#[derive(Debug, Serialize)]
struct MeResponse {
    user: UserResponse,
    active_tenant: TenantMembershipResponse,
}

#[derive(Debug, Serialize, Deserialize)]
struct TaskListResponse {
    data: Vec<TaskResponse>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
struct HealthResponse<'a> {
    status: &'a str,
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
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/refresh", post(refresh))
        .route("/auth/logout", post(logout))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_rate_limit_middleware,
        ));

    let protected_routes = Router::new()
        .route("/me", get(me))
        .route("/me/tenants", get(list_my_tenants))
        .route("/tasks", get(list_tasks).post(create_task))
        .route(
            "/tasks/:task_id",
            get(get_task).patch(update_task).delete(delete_task),
        )
        .route("/exports/tasks", post(create_export))
        .route("/jobs/:job_id", get(get_job))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            protected_middleware,
        ));

    let cors = build_cors(&state)?;

    Ok(Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/metrics", get(metrics))
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

async fn healthz() -> Json<HealthResponse<'static>> {
    Json(HealthResponse { status: "ok" })
}

async fn readyz(State(state): State<AppState>) -> AppResult<Json<HealthResponse<'static>>> {
    state.db.health_check().await?;
    state.cache.ping().await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

async fn register(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> AppResult<(StatusCode, Json<AuthResponse>)> {
    let email = normalize_email(&payload.email)?;
    validate_password(&payload.password)?;
    let tenant_name = payload
        .tenant_name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{} Workspace", email.split('@').next().unwrap_or("Team")));

    let password_hash = state.auth.hash_password(&payload.password)?;
    let (user, membership) = state
        .db
        .create_user_with_tenant(&email, &password_hash, &tenant_name)
        .await?;

    let refresh_token_id = Uuid::new_v4();
    state
        .db
        .create_refresh_token(
            refresh_token_id,
            user.id,
            membership.tenant_id,
            refresh_expiry(&state)?,
        )
        .await?;

    let tokens = state
        .auth
        .issue_token_pair(&user, &membership, refresh_token_id)?;

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            expires_in_seconds: tokens.expires_in_seconds,
            user: UserResponse::from(&user),
            active_tenant: TenantMembershipResponse::from(&membership),
        }),
    ))
}

async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let email = normalize_email(&payload.email)?;
    let user = state
        .db
        .get_user_by_email(&email)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid credentials".into()))?;
    state
        .auth
        .verify_password(&payload.password, &user.password_hash)?;

    let membership = resolve_membership(&state, user.id, payload.tenant_id).await?;
    let refresh_token_id = Uuid::new_v4();
    state
        .db
        .create_refresh_token(
            refresh_token_id,
            user.id,
            membership.tenant_id,
            refresh_expiry(&state)?,
        )
        .await?;

    let tokens = state
        .auth
        .issue_token_pair(&user, &membership, refresh_token_id)?;

    Ok(Json(AuthResponse {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_in_seconds: tokens.expires_in_seconds,
        user: UserResponse::from(&user),
        active_tenant: TenantMembershipResponse::from(&membership),
    }))
}

async fn refresh(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> AppResult<Json<AuthResponse>> {
    let claims = state.auth.decode_refresh_token(&payload.refresh_token)?;
    let refresh_token_id = parse_uuid(&claims.jti, "refresh token id")?;
    let user_id = parse_uuid(&claims.sub, "user id")?;
    let recorded = state
        .db
        .get_refresh_token(refresh_token_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("refresh token not found".into()))?;

    if recorded.revoked_at.is_some() || recorded.expires_at <= Utc::now() {
        return Err(AppError::Unauthorized(
            "refresh token is expired or revoked".into(),
        ));
    }

    let user = state.db.get_user_by_id(user_id).await?;
    let membership = resolve_membership(
        &state,
        user.id,
        payload.tenant_id.or(Some(recorded.tenant_id)),
    )
    .await?;
    let next_refresh_id = Uuid::new_v4();

    state
        .db
        .rotate_refresh_token(
            refresh_token_id,
            next_refresh_id,
            user.id,
            membership.tenant_id,
            refresh_expiry(&state)?,
        )
        .await?;

    let tokens = state
        .auth
        .issue_token_pair(&user, &membership, next_refresh_id)?;

    Ok(Json(AuthResponse {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_in_seconds: tokens.expires_in_seconds,
        user: UserResponse::from(&user),
        active_tenant: TenantMembershipResponse::from(&membership),
    }))
}

async fn logout(
    State(state): State<AppState>,
    Json(payload): Json<LogoutRequest>,
) -> AppResult<StatusCode> {
    let claims = state.auth.decode_refresh_token(&payload.refresh_token)?;
    let refresh_token_id = parse_uuid(&claims.jti, "refresh token id")?;
    state.db.revoke_refresh_token(refresh_token_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn me(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<MeResponse>> {
    let db_user = state.db.get_user_by_id(user.user_id).await?;
    let membership = state
        .db
        .get_membership(user.user_id, user.tenant_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("membership not found".into()))?;

    Ok(Json(MeResponse {
        user: UserResponse::from(&db_user),
        active_tenant: TenantMembershipResponse::from(&membership),
    }))
}

async fn list_my_tenants(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<Vec<TenantMembershipResponse>>> {
    let memberships = state.db.list_memberships(user.user_id).await?;
    Ok(Json(
        memberships
            .iter()
            .map(TenantMembershipResponse::from)
            .collect(),
    ))
}

async fn list_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<TaskListQuery>,
) -> AppResult<Json<TaskListResponse>> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let filters = query.clone().into_filters()?;
    let cursor = query.cursor.as_deref().map(Cursor::decode).transpose()?;
    let version = state.cache.tenant_cache_version(user.tenant_id).await?;
    let cache_payload = json!({
        "tenant_id": user.tenant_id,
        "limit": limit,
        "cursor": query.cursor,
        "filters": filters,
    });
    let cache_key = state
        .cache
        .task_list_cache_key(user.tenant_id, version, &cache_payload)?;

    if let Some(cached) = state.cache.get_json::<TaskListResponse>(&cache_key).await? {
        return Ok(Json(cached));
    }

    let tasks = state
        .db
        .list_tasks(user.tenant_id, &filters, cursor.as_ref(), limit)
        .await?;
    let next_cursor = tasks.next_cursor.map(|value| value.encode()).transpose()?;
    let response = TaskListResponse {
        data: tasks.tasks.iter().map(TaskResponse::from).collect(),
        next_cursor,
    };

    state
        .cache
        .set_json(&cache_key, &response, state.config.cache_ttl())
        .await?;

    Ok(Json(response))
}

async fn create_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(payload): Json<TaskPayload>,
) -> AppResult<(StatusCode, Json<Value>)> {
    ensure_task_write_role(&user.role)?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let cache_key = state
        .cache
        .idempotency_key(user.tenant_id, "tasks:create", idempotency_key);

    if let Some(response) = replay_idempotent(&state, &cache_key).await? {
        return Ok(response);
    }

    if !state
        .cache
        .claim_idempotency_key(&cache_key, state.config.idempotency_ttl())
        .await?
    {
        return match replay_idempotent(&state, &cache_key).await? {
            Some(response) => Ok(response),
            None => Err(AppError::Conflict(
                "request with this idempotency key is still in progress".into(),
            )),
        };
    }

    let input = CreateTaskInput {
        title: payload.title,
        description: payload.description,
        status: normalize_optional_choice(payload.status),
        priority: normalize_optional_choice(payload.priority),
        assignee_id: payload.assignee_id,
        due_at: payload.due_at,
    }
    .validate()?;

    match state
        .db
        .create_task(user.tenant_id, user.user_id, input)
        .await
    {
        Ok(task) => {
            state
                .cache
                .bump_tenant_cache_version(user.tenant_id)
                .await?;
            let body = serde_json::to_value(TaskResponse::from(&task)).map_err(|error| {
                AppError::internal(format!("failed to serialize task: {error}"))
            })?;
            let stored = StoredResponse {
                status: StatusCode::CREATED.as_u16(),
                body: body.clone(),
            };
            state
                .cache
                .store_idempotency_response(&cache_key, &stored, state.config.idempotency_ttl())
                .await?;
            Ok((StatusCode::CREATED, Json(body)))
        }
        Err(error) => {
            state.cache.delete_key(&cache_key).await?;
            Err(error)
        }
    }
}

async fn get_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
) -> AppResult<Json<TaskResponse>> {
    let version = state.cache.tenant_cache_version(user.tenant_id).await?;
    let cache_key = state
        .cache
        .task_detail_cache_key(user.tenant_id, version, task_id);

    if let Some(cached) = state.cache.get_json::<TaskResponse>(&cache_key).await? {
        return Ok(Json(cached));
    }

    let task = state
        .db
        .get_task(user.tenant_id, task_id)
        .await?
        .ok_or_else(|| AppError::NotFound("task not found".into()))?;
    let response = TaskResponse::from(&task);
    state
        .cache
        .set_json(&cache_key, &response, state.config.cache_ttl())
        .await?;
    Ok(Json(response))
}

async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
    Json(payload): Json<TaskPatchPayload>,
) -> AppResult<Json<TaskResponse>> {
    ensure_task_write_role(&user.role)?;
    let input = UpdateTaskInput {
        title: payload.title,
        description: payload.description,
        status: normalize_optional_choice(payload.status),
        priority: normalize_optional_choice(payload.priority),
        assignee_id: payload.assignee_id,
        due_at: payload.due_at,
    }
    .validate()?;

    let task = state
        .db
        .update_task(user.tenant_id, task_id, user.user_id, input)
        .await?;
    state
        .cache
        .bump_tenant_cache_version(user.tenant_id)
        .await?;
    Ok(Json(TaskResponse::from(&task)))
}

async fn delete_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_admin_role(&user.role)?;
    state
        .db
        .delete_task(user.tenant_id, task_id, user.user_id)
        .await?;
    state
        .cache
        .bump_tenant_cache_version(user.tenant_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn create_export(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(payload): Json<ExportRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    ensure_admin_role(&user.role)?;
    let idempotency_key = required_idempotency_key(&headers)?;
    let cache_key = state
        .cache
        .idempotency_key(user.tenant_id, "exports:create", idempotency_key);

    if let Some(response) = replay_idempotent(&state, &cache_key).await? {
        return Ok(response);
    }

    if !state
        .cache
        .claim_idempotency_key(&cache_key, state.config.idempotency_ttl())
        .await?
    {
        return match replay_idempotent(&state, &cache_key).await? {
            Some(response) => Ok(response),
            None => Err(AppError::Conflict(
                "request with this idempotency key is still in progress".into(),
            )),
        };
    }

    let filters = payload.into_filters()?;
    let job = match state
        .db
        .create_job(
            Some(user.tenant_id),
            "task_export",
            json!({
                "tenant_id": user.tenant_id,
                "requested_by": user.user_id,
                "filters": filters.export_payload(),
            }),
            state.config.max_job_attempts,
        )
        .await
    {
        Ok(job) => {
            state.cache.enqueue_job(job.id).await?;
            job
        }
        Err(error) => {
            state.cache.delete_key(&cache_key).await?;
            return Err(error);
        }
    };

    let body = serde_json::to_value(JobResponse::from(&job))
        .map_err(|error| AppError::internal(format!("failed to serialize job: {error}")))?;
    let stored = StoredResponse {
        status: StatusCode::ACCEPTED.as_u16(),
        body: body.clone(),
    };
    state
        .cache
        .store_idempotency_response(&cache_key, &stored, state.config.idempotency_ttl())
        .await?;

    Ok((StatusCode::ACCEPTED, Json(body)))
}

async fn get_job(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(job_id): Path<Uuid>,
) -> AppResult<Json<JobResponse>> {
    let job = state
        .db
        .get_job(job_id)
        .await?
        .ok_or_else(|| AppError::NotFound("job not found".into()))?;

    if job.tenant_id != Some(user.tenant_id) {
        return Err(AppError::NotFound("job not found".into()));
    }

    Ok(Json(JobResponse::from(&job)))
}

async fn auth_rate_limit_middleware(
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

async fn protected_middleware(
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
        .ok_or_else(|| AppError::Unauthorized("membership not found".into()))?;

    let auth_user = AuthenticatedUser {
        user_id,
        tenant_id,
        role: membership.role.clone(),
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

fn build_cors(state: &AppState) -> AppResult<CorsLayer> {
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

fn normalize_email(email: &str) -> AppResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') {
        return Err(AppError::Validation("email must be valid".into()));
    }
    Ok(email)
}

fn validate_password(password: &str) -> AppResult<()> {
    if password.len() < 8 {
        return Err(AppError::Validation(
            "password must be at least 8 characters".into(),
        ));
    }
    Ok(())
}

async fn resolve_membership(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Option<Uuid>,
) -> AppResult<MembershipRecord> {
    match tenant_id {
        Some(tenant_id) => state
            .db
            .get_membership(user_id, tenant_id)
            .await?
            .ok_or_else(|| AppError::Unauthorized("membership not found".into())),
        None => state
            .db
            .get_default_membership(user_id)
            .await?
            .ok_or_else(|| AppError::Unauthorized("membership not found".into())),
    }
}

fn refresh_expiry(state: &AppState) -> AppResult<DateTime<Utc>> {
    let ttl = ChronoDuration::from_std(state.config.refresh_token_ttl())
        .map_err(|error| AppError::internal(format!("invalid refresh token ttl: {error}")))?;
    Ok(Utc::now() + ttl)
}

fn parse_uuid(value: &str, label: &str) -> AppResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::Unauthorized(format!("invalid {label}: {error}")))
}

fn normalize_optional_choice(value: Option<String>) -> Option<String> {
    value.map(|value| value.trim().to_ascii_lowercase())
}

fn ensure_task_write_role(role: &str) -> AppResult<()> {
    match role {
        "owner" | "admin" | "member" => Ok(()),
        _ => Err(AppError::Forbidden(
            "role is not allowed to modify tasks".into(),
        )),
    }
}

fn ensure_admin_role(role: &str) -> AppResult<()> {
    match role {
        "owner" | "admin" => Ok(()),
        _ => Err(AppError::Forbidden(
            "owner or admin role required for this action".into(),
        )),
    }
}

fn required_idempotency_key(headers: &HeaderMap) -> AppResult<&str> {
    headers
        .get("Idempotency-Key")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Validation("Idempotency-Key header is required".into()))
}

async fn replay_idempotent(
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

fn bearer_token(headers: &HeaderMap) -> AppResult<&str> {
    let value = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Unauthorized("missing authorization header".into()))?;
    value
        .strip_prefix("Bearer ")
        .ok_or_else(|| AppError::Unauthorized("expected bearer token".into()))
}

fn client_identifier(request: &Request) -> String {
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

fn attach_rate_limit_headers(
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

fn rate_limit_response(decision: crate::cache::RateLimitDecision) -> Response {
    let mut response = AppError::RateLimited("rate limit exceeded".into()).into_response();
    let _ = attach_rate_limit_headers(&mut response, &decision);
    response
}

impl TaskListQuery {
    fn into_filters(self) -> AppResult<TaskFilters> {
        TaskFilters {
            status: normalize_optional_choice(self.status),
            priority: normalize_optional_choice(self.priority),
            assignee_id: self.assignee_id,
            due_before: parse_optional_datetime(self.due_before, "due_before")?,
            due_after: parse_optional_datetime(self.due_after, "due_after")?,
            updated_after: parse_optional_datetime(self.updated_after, "updated_after")?,
            q: self.q.filter(|value| !value.trim().is_empty()),
        }
        .validate()
    }
}

impl ExportRequest {
    fn into_filters(self) -> AppResult<TaskFilters> {
        TaskFilters {
            status: normalize_optional_choice(self.status),
            priority: normalize_optional_choice(self.priority),
            assignee_id: self.assignee_id,
            due_before: parse_optional_datetime(self.due_before, "due_before")?,
            due_after: parse_optional_datetime(self.due_after, "due_after")?,
            updated_after: parse_optional_datetime(self.updated_after, "updated_after")?,
            q: self.q.filter(|value| !value.trim().is_empty()),
        }
        .validate()
    }
}

fn parse_optional_datetime(value: Option<String>, field: &str) -> AppResult<Option<DateTime<Utc>>> {
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

#[cfg(test)]
mod tests {
    use super::{normalize_optional_choice, parse_optional_datetime};
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
