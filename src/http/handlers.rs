use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::CONTENT_TYPE;
use axum::response::IntoResponse;
use serde_json::Value;
use uuid::Uuid;

use crate::cache::StoredResponse;
use crate::domain::{
    CreateTaskInput, DashboardSummary, JobResponse, TaskResponse, TenantMemberResponse,
    TenantMembershipResponse, UpdateTaskInput, UserResponse, validate_task_priority,
    validate_task_status,
};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;
use crate::services::{auth as auth_service, jobs as jobs_service, tasks as task_service};
use crate::state::AppState;

use super::AuthenticatedUser;
use super::dto::{
    AuthResponse, ExportRequest, HealthResponse, LoginRequest, LogoutRequest, MeResponse,
    RefreshRequest, RegisterRequest, SwitchTenantRequest, TaskListQuery, TaskListResponse,
    TaskPatchPayload, TaskPayload,
};
use super::helpers::{
    ensure_admin_role, ensure_task_write_role, normalize_email, normalize_optional_choice,
    replay_idempotent, required_idempotency_key, validate_password,
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
    let session =
        auth_service::register(&state, &email, &payload.password, payload.tenant_name).await?;

    Ok((
        StatusCode::CREATED,
        Json(AuthResponse {
            access_token: session.access_token,
            refresh_token: session.refresh_token,
            expires_in_seconds: session.expires_in_seconds,
            user: UserResponse::from(&session.user),
            active_tenant: TenantMembershipResponse::try_from(&session.membership)?,
        }),
    ))
}

pub(super) async fn login(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> AppResult<Json<AuthResponse>> {
    let email = normalize_email(&payload.email)?;
    let session = auth_service::login(&state, &email, &payload.password, payload.tenant_id).await?;

    Ok(Json(AuthResponse {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        expires_in_seconds: session.expires_in_seconds,
        user: UserResponse::from(&session.user),
        active_tenant: TenantMembershipResponse::try_from(&session.membership)?,
    }))
}

pub(super) async fn refresh(
    State(state): State<AppState>,
    Json(payload): Json<RefreshRequest>,
) -> AppResult<Json<AuthResponse>> {
    let session = auth_service::refresh(&state, &payload.refresh_token, payload.tenant_id).await?;

    Ok(Json(AuthResponse {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        expires_in_seconds: session.expires_in_seconds,
        user: UserResponse::from(&session.user),
        active_tenant: TenantMembershipResponse::try_from(&session.membership)?,
    }))
}

pub(super) async fn logout(
    State(state): State<AppState>,
    Json(payload): Json<LogoutRequest>,
) -> AppResult<StatusCode> {
    auth_service::logout(&state, &payload.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn switch_tenant(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(payload): Json<SwitchTenantRequest>,
) -> AppResult<Json<AuthResponse>> {
    let session = auth_service::switch_tenant(&state, user.user_id, payload.tenant_id).await?;

    Ok(Json(AuthResponse {
        access_token: session.access_token,
        refresh_token: session.refresh_token,
        expires_in_seconds: session.expires_in_seconds,
        user: UserResponse::from(&session.user),
        active_tenant: TenantMembershipResponse::try_from(&session.membership)?,
    }))
}

pub(super) async fn me(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<MeResponse>> {
    let profile = auth_service::me(&state, user.user_id, user.tenant_id).await?;

    Ok(Json(MeResponse {
        user: UserResponse::from(&profile.user),
        active_tenant: TenantMembershipResponse::try_from(&profile.membership)?,
    }))
}

pub(super) async fn list_my_tenants(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<Vec<TenantMembershipResponse>>> {
    let memberships = auth_service::list_tenants(&state, user.user_id).await?;
    Ok(Json(
        memberships
            .iter()
            .map(TenantMembershipResponse::try_from)
            .collect::<AppResult<Vec<_>>>()?,
    ))
}

pub(super) async fn list_tenant_members(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<TenantMemberResponse>>> {
    let members = auth_service::list_tenant_members(&state, user.tenant_id, tenant_id).await?;

    Ok(Json(
        members
            .iter()
            .map(TenantMemberResponse::try_from)
            .collect::<AppResult<Vec<_>>>()?,
    ))
}

pub(super) async fn dashboard_summary(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<DashboardSummary>> {
    let summary = task_service::dashboard_summary(&state, user.tenant_id).await?;
    Ok(Json(summary))
}

pub(super) async fn list_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<TaskListQuery>,
) -> AppResult<Json<TaskListResponse>> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let filters = query.clone().into_filters()?;
    let cursor = query.cursor.as_deref().map(Cursor::decode).transpose()?;
    let page = task_service::list_tasks_cached(
        &state,
        user.tenant_id,
        &filters,
        query.cursor.as_deref(),
        cursor.as_ref(),
        limit,
    )
    .await?;

    Ok(Json(TaskListResponse {
        data: page.data,
        next_cursor: page.next_cursor,
    }))
}

pub(super) async fn create_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(payload): Json<TaskPayload>,
) -> AppResult<(StatusCode, Json<Value>)> {
    ensure_task_write_role(user.role)?;
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
        status: normalize_optional_choice(payload.status)
            .map(|value| validate_task_status(&value))
            .transpose()?,
        priority: normalize_optional_choice(payload.priority)
            .map(|value| validate_task_priority(&value))
            .transpose()?,
        assignee_id: payload.assignee_id,
        due_at: payload.due_at,
    }
    .validate()?;

    match task_service::create_task(&state, user.tenant_id, user.user_id, input).await {
        Ok(task) => {
            let body = serde_json::to_value(TaskResponse::try_from(&task)?).map_err(|error| {
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
    let task = task_service::get_task_cached(&state, user.tenant_id, task_id).await?;
    Ok(Json(task))
}

pub(super) async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
    Json(payload): Json<TaskPatchPayload>,
) -> AppResult<Json<TaskResponse>> {
    ensure_task_write_role(user.role)?;
    let input = UpdateTaskInput {
        title: payload.title,
        description: payload.description,
        status: normalize_optional_choice(payload.status)
            .map(|value| validate_task_status(&value))
            .transpose()?,
        priority: normalize_optional_choice(payload.priority)
            .map(|value| validate_task_priority(&value))
            .transpose()?,
        assignee_id: payload.assignee_id,
        due_at: payload.due_at,
    }
    .validate()?;

    let task =
        task_service::update_task(&state, user.tenant_id, task_id, user.user_id, input).await?;
    Ok(Json(TaskResponse::try_from(&task)?))
}

pub(super) async fn delete_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_admin_role(user.role)?;
    task_service::delete_task(&state, user.tenant_id, task_id, user.user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn create_export(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    headers: HeaderMap,
    Json(payload): Json<ExportRequest>,
) -> AppResult<(StatusCode, Json<Value>)> {
    ensure_admin_role(user.role)?;
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
    let job = match jobs_service::create_export_job(&state, user.tenant_id, user.user_id, &filters)
        .await
    {
        Ok(job) => job,
        Err(error) => {
            state.cache.delete_key(&cache_key).await?;
            return Err(error);
        }
    };

    let body = jobs_service::job_response_value(&job)?;
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
    let job = jobs_service::get_tenant_job(&state, job_id, user.tenant_id).await?;

    Ok(Json(JobResponse::try_from(&job)?))
}
