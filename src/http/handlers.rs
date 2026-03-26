use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use chrono::Utc;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::cache::StoredResponse;
use crate::domain::{
    CreateTaskInput, JobResponse, TaskResponse, TenantMembershipResponse, UpdateTaskInput,
    UserResponse,
};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;
use crate::state::AppState;

use super::AuthenticatedUser;
use super::dto::{
    AuthResponse, ExportRequest, HealthResponse, LoginRequest, LogoutRequest, MeResponse,
    RefreshRequest, RegisterRequest, TaskListQuery, TaskListResponse, TaskPatchPayload,
    TaskPayload,
};
use super::helpers::{
    ensure_admin_role, ensure_task_write_role, normalize_email, normalize_optional_choice,
    parse_uuid, refresh_expiry, replay_idempotent, required_idempotency_key, resolve_membership,
    validate_password,
};

pub(super) async fn healthz() -> Json<HealthResponse<'static>> {
    Json(HealthResponse { status: "ok" })
}

pub(super) async fn readyz(
    State(state): State<AppState>,
) -> AppResult<Json<HealthResponse<'static>>> {
    state.db.health_check().await?;
    state.cache.ping().await?;
    Ok(Json(HealthResponse { status: "ready" }))
}

pub(super) async fn metrics(State(state): State<AppState>) -> impl IntoResponse {
    (
        [(CONTENT_TYPE, "text/plain; version=0.0.4")],
        state.metrics.render(),
    )
}

pub(super) async fn register(
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

pub(super) async fn login(
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

pub(super) async fn refresh(
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

pub(super) async fn logout(
    State(state): State<AppState>,
    Json(payload): Json<LogoutRequest>,
) -> AppResult<StatusCode> {
    let claims = state.auth.decode_refresh_token(&payload.refresh_token)?;
    let refresh_token_id = parse_uuid(&claims.jti, "refresh token id")?;
    state.db.revoke_refresh_token(refresh_token_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn me(
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

pub(super) async fn list_my_tenants(
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

pub(super) async fn list_tasks(
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

pub(super) async fn create_task(
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

pub(super) async fn get_task(
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

pub(super) async fn update_task(
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

pub(super) async fn delete_task(
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

pub(super) async fn create_export(
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

pub(super) async fn get_job(
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
