use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::response::IntoResponse;
use serde_json::Value;
use uuid::Uuid;

use crate::cache::StoredResponse;
use crate::domain::{
    CreateProjectInput, CreateTaskInput, DashboardSummary, InvitationResponse, JobResponse,
    JobResultResponse, PaginatedAuditEvents, ProjectResponse, ProjectSummary, TaskAuditResponse,
    TaskResponse, TenantMemberResponse, TenantMembershipResponse, UpdateProjectInput,
    UpdateTaskInput, UserResponse, validate_role, validate_task_priority, validate_task_status,
};
use crate::error::{AppError, AppResult};
use crate::pagination::{AuditCursor, Cursor};
use crate::services::{
    account as account_service, audit as audit_service, auth as auth_service, jobs as jobs_service,
    memberships as membership_service, projects as project_service, tasks as task_service,
};
use crate::state::AppState;
use crate::storage::ArtifactStore;

use super::AuthenticatedUser;
use super::dto::{
    AuditListQuery, AuthResponse, ChangeEmailPayload, ChangePasswordPayload, ExportRequest,
    HealthResponse, InvitationAcceptPayload, InvitationCreatePayload, InvitationCreateResponse,
    LoginRequest, LogoutRequest, MeResponse, MemberRolePayload, PasswordResetConfirmPayload,
    PasswordResetRequestPayload, ProjectPatchPayload, ProjectPayload, RefreshRequest,
    RegisterRequest, ResendVerificationPayload, SwitchTenantRequest, TaskAuditListResponse,
    TaskAuditQuery, TaskListQuery, TaskListResponse, TaskPatchPayload, TaskPayload,
    VerifyEmailPayload,
};
use super::helpers::{
    bearer_token, ensure_active_tenant, ensure_admin_role, ensure_task_write_role, normalize_email,
    normalize_optional_choice, replay_idempotent, required_idempotency_key, validate_password,
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
    headers: HeaderMap,
    Json(payload): Json<LogoutRequest>,
) -> AppResult<StatusCode> {
    let header_access_token = bearer_token(&headers).ok();
    let access_token = payload.access_token.as_deref().or(header_access_token);
    auth_service::logout(&state, &payload.refresh_token, access_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn verify_email(
    State(state): State<AppState>,
    Json(payload): Json<VerifyEmailPayload>,
) -> AppResult<StatusCode> {
    let token = payload.token.trim();
    if token.is_empty() {
        return Err(AppError::Validation("token must not be empty".into()));
    }
    account_service::verify_email(&state, token).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn resend_verification(
    State(state): State<AppState>,
    Json(payload): Json<ResendVerificationPayload>,
) -> AppResult<StatusCode> {
    let email = normalize_email(&payload.email)?;
    account_service::resend_verification(&state, &email).await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn request_password_reset(
    State(state): State<AppState>,
    Json(payload): Json<PasswordResetRequestPayload>,
) -> AppResult<StatusCode> {
    let email = normalize_email(&payload.email)?;
    account_service::request_password_reset(&state, &email).await?;
    Ok(StatusCode::ACCEPTED)
}

pub(super) async fn confirm_password_reset(
    State(state): State<AppState>,
    Json(payload): Json<PasswordResetConfirmPayload>,
) -> AppResult<StatusCode> {
    let token = payload.token.trim();
    if token.is_empty() {
        return Err(AppError::Validation("token must not be empty".into()));
    }
    validate_password(&payload.new_password)?;
    account_service::confirm_password_reset(&state, token, &payload.new_password).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn change_password(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(payload): Json<ChangePasswordPayload>,
) -> AppResult<StatusCode> {
    validate_password(&payload.new_password)?;
    account_service::change_password(
        &state,
        user.user_id,
        &payload.current_password,
        &payload.new_password,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn change_email(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(payload): Json<ChangeEmailPayload>,
) -> AppResult<Json<UserResponse>> {
    let email = normalize_email(&payload.new_email)?;
    let updated =
        account_service::change_email(&state, user.user_id, &payload.current_password, &email)
            .await?;
    Ok(Json(UserResponse::from(&updated)))
}

pub(super) async fn list_audit_events(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Query(query): Query<AuditListQuery>,
) -> AppResult<Json<PaginatedAuditEvents>> {
    ensure_admin_role(user.role)?;
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let cursor = query
        .cursor
        .as_deref()
        .map(AuditCursor::decode)
        .transpose()?;

    let page = audit_service::list_events(&state, user.tenant_id, cursor.as_ref(), limit).await?;
    Ok(Json(page))
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

pub(super) async fn create_invitation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(tenant_id): Path<Uuid>,
    Json(payload): Json<InvitationCreatePayload>,
) -> AppResult<(StatusCode, Json<InvitationCreateResponse>)> {
    ensure_active_tenant(user.tenant_id, tenant_id)?;
    ensure_admin_role(user.role)?;
    let email = normalize_email(&payload.email)?;
    let role = validate_role(payload.role.trim().to_ascii_lowercase().as_str())?;

    let created = membership_service::create_invitation(
        &state,
        tenant_id,
        user.role,
        user.user_id,
        &email,
        role,
    )
    .await?;

    Ok((
        StatusCode::CREATED,
        Json(InvitationCreateResponse {
            invitation: InvitationResponse::try_from(&created.invitation)?,
            token: created.token,
        }),
    ))
}

pub(super) async fn list_invitations(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(tenant_id): Path<Uuid>,
) -> AppResult<Json<Vec<InvitationResponse>>> {
    ensure_active_tenant(user.tenant_id, tenant_id)?;
    ensure_admin_role(user.role)?;
    let invitations = membership_service::list_invitations(&state, tenant_id).await?;

    Ok(Json(
        invitations
            .iter()
            .map(InvitationResponse::try_from)
            .collect::<AppResult<Vec<_>>>()?,
    ))
}

pub(super) async fn revoke_invitation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((tenant_id, invitation_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    ensure_active_tenant(user.tenant_id, tenant_id)?;
    ensure_admin_role(user.role)?;
    membership_service::revoke_invitation(&state, tenant_id, user.user_id, invitation_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn accept_invitation(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(tenant_id): Path<Uuid>,
    Json(payload): Json<InvitationAcceptPayload>,
) -> AppResult<Json<TenantMembershipResponse>> {
    let token = payload.token.trim();
    if token.is_empty() {
        return Err(AppError::Validation("token must not be empty".into()));
    }

    let membership =
        membership_service::accept_invitation(&state, tenant_id, user.user_id, token).await?;
    Ok(Json(TenantMembershipResponse::try_from(&membership)?))
}

pub(super) async fn update_member_role(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((tenant_id, member_id)): Path<(Uuid, Uuid)>,
    Json(payload): Json<MemberRolePayload>,
) -> AppResult<Json<TenantMemberResponse>> {
    ensure_active_tenant(user.tenant_id, tenant_id)?;
    ensure_admin_role(user.role)?;
    let role = validate_role(payload.role.trim().to_ascii_lowercase().as_str())?;

    let member = membership_service::update_member_role(
        &state,
        tenant_id,
        user.role,
        user.user_id,
        member_id,
        role,
    )
    .await?;
    Ok(Json(TenantMemberResponse::try_from(&member)?))
}

pub(super) async fn remove_member(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path((tenant_id, member_id)): Path<(Uuid, Uuid)>,
) -> AppResult<StatusCode> {
    ensure_active_tenant(user.tenant_id, tenant_id)?;
    ensure_admin_role(user.role)?;
    membership_service::remove_member(&state, tenant_id, user.role, user.user_id, member_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn list_projects(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
) -> AppResult<Json<Vec<ProjectResponse>>> {
    let projects = project_service::list_projects(&state, user.tenant_id).await?;
    Ok(Json(projects.iter().map(ProjectResponse::from).collect()))
}

pub(super) async fn create_project(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Json(payload): Json<ProjectPayload>,
) -> AppResult<(StatusCode, Json<ProjectResponse>)> {
    ensure_admin_role(user.role)?;
    let input = CreateProjectInput {
        name: payload.name,
        description: payload.description,
    }
    .validate()?;

    let project =
        project_service::create_project(&state, user.tenant_id, user.user_id, input).await?;
    Ok((StatusCode::CREATED, Json(ProjectResponse::from(&project))))
}

pub(super) async fn get_project(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(project_id): Path<Uuid>,
) -> AppResult<Json<ProjectResponse>> {
    let project = project_service::get_project(&state, user.tenant_id, project_id).await?;
    Ok(Json(ProjectResponse::from(&project)))
}

pub(super) async fn get_project_summary(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(project_id): Path<Uuid>,
) -> AppResult<Json<ProjectSummary>> {
    let summary = project_service::project_summary(&state, user.tenant_id, project_id).await?;
    Ok(Json(summary))
}

pub(super) async fn list_project_tasks(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(project_id): Path<Uuid>,
    Query(mut query): Query<TaskListQuery>,
) -> AppResult<Json<TaskListResponse>> {
    project_service::get_project(&state, user.tenant_id, project_id).await?;

    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    query.project_id = Some(project_id);

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

pub(super) async fn update_project(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<ProjectPatchPayload>,
) -> AppResult<Json<ProjectResponse>> {
    ensure_admin_role(user.role)?;
    let input = UpdateProjectInput {
        name: payload.name,
        description: payload.description,
    }
    .validate()?;

    let project =
        project_service::update_project(&state, user.tenant_id, project_id, user.user_id, input)
            .await?;
    Ok(Json(ProjectResponse::from(&project)))
}

pub(super) async fn delete_project(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(project_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    ensure_admin_role(user.role)?;
    project_service::delete_project(&state, user.tenant_id, user.user_id, project_id).await?;
    Ok(StatusCode::NO_CONTENT)
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
        project_id: payload.project_id,
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

pub(super) async fn list_task_audit(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
    Query(query): Query<TaskAuditQuery>,
) -> AppResult<Json<TaskAuditListResponse>> {
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let cursor = query
        .cursor
        .as_deref()
        .map(AuditCursor::decode)
        .transpose()?;
    let page =
        task_service::list_task_audit(&state, user.tenant_id, task_id, cursor.as_ref(), limit)
            .await?;

    Ok(Json(TaskAuditListResponse {
        data: page.entries.iter().map(TaskAuditResponse::from).collect(),
        next_cursor: page.next_cursor.map(|value| value.encode()).transpose()?,
    }))
}

pub(super) async fn update_task(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(task_id): Path<Uuid>,
    Json(payload): Json<TaskPatchPayload>,
) -> AppResult<Json<TaskResponse>> {
    ensure_task_write_role(user.role)?;
    let input = UpdateTaskInput {
        project_id: payload.project_id,
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

    let format = payload.export_format()?;
    let filters = payload.into_filters()?;
    let job = match jobs_service::create_export_job(
        &state,
        user.tenant_id,
        user.user_id,
        &filters,
        format,
    )
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

pub(super) async fn get_job_result(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(job_id): Path<Uuid>,
) -> AppResult<Json<JobResultResponse>> {
    let result = jobs_service::get_tenant_job_result(&state, job_id, user.tenant_id).await?;
    Ok(Json(result))
}

pub(super) async fn download_job_artifact(
    State(state): State<AppState>,
    Extension(user): Extension<AuthenticatedUser>,
    Path(job_id): Path<Uuid>,
) -> AppResult<impl IntoResponse> {
    let result = jobs_service::get_tenant_job_result(&state, job_id, user.tenant_id).await?;
    let artifact = result
        .result
        .get("artifact")
        .ok_or_else(|| AppError::NotFound("job has no artifact".into()))?;
    let key = artifact
        .get("key")
        .and_then(Value::as_str)
        .ok_or_else(|| AppError::NotFound("job has no artifact".into()))?;
    let content_type = artifact
        .get("content_type")
        .and_then(Value::as_str)
        .unwrap_or("application/octet-stream")
        .to_owned();

    let bytes = state
        .storage
        .get(key)
        .await?
        .ok_or_else(|| AppError::NotFound("artifact is no longer available".into()))?;

    let filename = key.rsplit('/').next().unwrap_or("export").to_owned();
    Ok((
        [
            (CONTENT_TYPE, content_type),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        bytes,
    ))
}
