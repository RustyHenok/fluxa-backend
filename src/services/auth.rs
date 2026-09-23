use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::domain::{MembershipRecord, TenantMemberRecord, UserRecord};
use crate::error::{AppError, AppResult};
use crate::services::{account, audit};
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct AuthSession {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: u64,
    pub user: UserRecord,
    pub membership: MembershipRecord,
}

#[derive(Debug, Clone)]
pub struct CurrentUserProfile {
    pub user: UserRecord,
    pub membership: MembershipRecord,
}

pub async fn register(
    state: &AppState,
    email: &str,
    password: &str,
    tenant_name: Option<String>,
) -> AppResult<AuthSession> {
    let tenant_name = tenant_name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{} Workspace", email.split('@').next().unwrap_or("Team")));

    let password_hash = state.auth.hash_password(password)?;
    let (user, membership) = state
        .db
        .create_user_with_tenant(email, &password_hash, &tenant_name)
        .await?;

    account::send_verification_email(state, &user).await;
    audit::record_event(
        state,
        Some(membership.tenant_id),
        Some(user.id),
        "user",
        Some(user.id),
        "user.registered",
        json!({}),
    )
    .await;

    issue_session(state, user, membership, Uuid::new_v4()).await
}

pub async fn login(
    state: &AppState,
    email: &str,
    password: &str,
    tenant_id: Option<Uuid>,
) -> AppResult<AuthSession> {
    let Some(user) = state.db.get_user_by_email(email).await? else {
        audit::record_event(
            state,
            None,
            None,
            "user",
            None,
            "auth.login_failed",
            json!({ "reason": "unknown_email" }),
        )
        .await;
        return Err(AppError::Unauthorized("invalid credentials".into()));
    };

    if let Err(error) = state.auth.verify_password(password, &user.password_hash) {
        audit::record_event(
            state,
            None,
            Some(user.id),
            "user",
            Some(user.id),
            "auth.login_failed",
            json!({ "reason": "invalid_password" }),
        )
        .await;
        return Err(error);
    }

    ensure_email_verified(state, &user)?;

    let membership = resolve_membership(state, user.id, tenant_id).await?;
    audit::record_event(
        state,
        Some(membership.tenant_id),
        Some(user.id),
        "user",
        Some(user.id),
        "auth.login_succeeded",
        json!({}),
    )
    .await;
    issue_session(state, user, membership, Uuid::new_v4()).await
}

/// Rejects users with unverified email addresses when
/// `REQUIRE_EMAIL_VERIFICATION` is enabled.
fn ensure_email_verified(state: &AppState, user: &UserRecord) -> AppResult<()> {
    if state.config.require_email_verification && user.email_verified_at.is_none() {
        return Err(AppError::Forbidden("email address is not verified".into()));
    }
    Ok(())
}

pub async fn refresh(
    state: &AppState,
    refresh_token: &str,
    tenant_id: Option<Uuid>,
) -> AppResult<AuthSession> {
    let claims = state.auth.decode_refresh_token(refresh_token)?;
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
    ensure_email_verified(state, &user)?;
    let membership =
        resolve_membership(state, user.id, tenant_id.or(Some(recorded.tenant_id))).await?;
    let next_refresh_id = Uuid::new_v4();

    state
        .db
        .rotate_refresh_token(
            refresh_token_id,
            next_refresh_id,
            user.id,
            membership.tenant_id,
            refresh_expiry(state)?,
        )
        .await?;

    let tokens = state
        .auth
        .issue_token_pair(&user, &membership, next_refresh_id)?;

    audit::record_event(
        state,
        Some(membership.tenant_id),
        Some(user.id),
        "user",
        Some(user.id),
        "auth.token_refreshed",
        json!({}),
    )
    .await;

    Ok(AuthSession {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_in_seconds: tokens.expires_in_seconds,
        user,
        membership,
    })
}

pub async fn logout(
    state: &AppState,
    refresh_token: &str,
    access_token: Option<&str>,
) -> AppResult<()> {
    let claims = state.auth.decode_refresh_token(refresh_token)?;
    let refresh_token_id = parse_uuid(&claims.jti, "refresh token id")?;
    let user_id = parse_uuid(&claims.sub, "user id").ok();
    state.db.revoke_refresh_token(refresh_token_id).await?;

    audit::record_event(
        state,
        None,
        user_id,
        "user",
        user_id,
        "auth.logged_out",
        json!({}),
    )
    .await;

    if let Some(access_token) = access_token {
        deny_access_token(state, access_token).await?;
    }

    Ok(())
}

/// Adds a still-valid access token to the Redis denylist for the remainder of
/// its lifetime. Invalid or already-expired tokens are ignored so logout stays
/// idempotent.
async fn deny_access_token(state: &AppState, access_token: &str) -> AppResult<()> {
    let Ok(claims) = state.auth.decode_access_token(access_token) else {
        return Ok(());
    };

    let remaining_seconds = claims.exp - Utc::now().timestamp();
    if remaining_seconds <= 0 {
        return Ok(());
    }

    state
        .cache
        .deny_access_token(
            &claims.jti,
            std::time::Duration::from_secs(remaining_seconds as u64),
        )
        .await
}

pub async fn switch_tenant(
    state: &AppState,
    user_id: Uuid,
    tenant_id: Uuid,
) -> AppResult<AuthSession> {
    let user = state.db.get_user_by_id(user_id).await?;
    let membership = state
        .db
        .get_membership(user_id, tenant_id)
        .await?
        .ok_or_else(|| AppError::NotFound("tenant membership not found".into()))?;

    issue_session(state, user, membership, Uuid::new_v4()).await
}

pub async fn me(state: &AppState, user_id: Uuid, tenant_id: Uuid) -> AppResult<CurrentUserProfile> {
    let user = state.db.get_user_by_id(user_id).await?;
    let membership = state
        .db
        .get_membership(user_id, tenant_id)
        .await?
        .ok_or_else(|| AppError::Unauthorized("membership not found".into()))?;

    Ok(CurrentUserProfile { user, membership })
}

pub async fn list_tenants(state: &AppState, user_id: Uuid) -> AppResult<Vec<MembershipRecord>> {
    state.db.list_memberships(user_id).await
}

pub async fn list_tenant_members(
    state: &AppState,
    active_tenant_id: Uuid,
    requested_tenant_id: Uuid,
) -> AppResult<Vec<TenantMemberRecord>> {
    if active_tenant_id != requested_tenant_id {
        return Err(AppError::NotFound("tenant not found".into()));
    }

    state.db.list_tenant_members(requested_tenant_id).await
}

async fn issue_session(
    state: &AppState,
    user: UserRecord,
    membership: MembershipRecord,
    refresh_token_id: Uuid,
) -> AppResult<AuthSession> {
    state
        .db
        .create_refresh_token(
            refresh_token_id,
            user.id,
            membership.tenant_id,
            refresh_expiry(state)?,
        )
        .await?;

    let tokens = state
        .auth
        .issue_token_pair(&user, &membership, refresh_token_id)?;

    Ok(AuthSession {
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_in_seconds: tokens.expires_in_seconds,
        user,
        membership,
    })
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

fn refresh_expiry(state: &AppState) -> AppResult<chrono::DateTime<Utc>> {
    let ttl = ChronoDuration::from_std(state.config.refresh_token_ttl())
        .map_err(|error| AppError::internal(format!("invalid refresh token ttl: {error}")))?;
    Ok(Utc::now() + ttl)
}

fn parse_uuid(value: &str, label: &str) -> AppResult<Uuid> {
    Uuid::parse_str(value)
        .map_err(|error| AppError::Unauthorized(format!("invalid {label}: {error}")))
}
