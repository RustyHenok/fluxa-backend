//! Account lifecycle flows: email verification, password reset, and
//! credential changes. Verification and reset tokens are opaque single-use
//! values delivered through the notifications outbox; only their hashes are
//! stored.

use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use tracing::warn;
use uuid::Uuid;

use crate::domain::{
    NewNotification, TOKEN_KIND_EMAIL_VERIFICATION, TOKEN_KIND_PASSWORD_RESET, UserRecord,
};
use crate::error::{AppError, AppResult};
use crate::notify::{KIND_EMAIL_VERIFICATION, KIND_PASSWORD_RESET};
use crate::services::audit;
use crate::state::AppState;
use crate::tokens::{generate_token, hash_token};

/// Enqueues an email-verification notification for the user, invalidating any
/// previously issued verification tokens. Best-effort: failures are logged so
/// registration and email changes never fail on outbox errors.
pub async fn send_verification_email(state: &AppState, user: &UserRecord) {
    if user.email_verified_at.is_some() {
        return;
    }

    if let Err(error) = create_and_enqueue_token(
        state,
        user,
        TOKEN_KIND_EMAIL_VERIFICATION,
        KIND_EMAIL_VERIFICATION,
        state.config.email_verification_ttl(),
    )
    .await
    {
        warn!("failed to enqueue verification email: {error}");
    }
}

/// Verifies an email address from a single-use token.
pub async fn verify_email(state: &AppState, token: &str) -> AppResult<()> {
    let record = state
        .db
        .consume_user_token(TOKEN_KIND_EMAIL_VERIFICATION, &hash_token(token))
        .await?;
    let user = state.db.mark_user_email_verified(record.user_id).await?;

    audit::record_event(
        state,
        None,
        Some(user.id),
        "user",
        Some(user.id),
        "account.email_verified",
        json!({}),
    )
    .await;

    Ok(())
}

/// Re-sends a verification email. Always succeeds from the caller's
/// perspective to avoid account enumeration.
pub async fn resend_verification(state: &AppState, email: &str) -> AppResult<()> {
    if let Some(user) = state.db.get_user_by_email(email).await? {
        send_verification_email(state, &user).await;
    }
    Ok(())
}

/// Starts a password reset. Always succeeds from the caller's perspective to
/// avoid account enumeration.
pub async fn request_password_reset(state: &AppState, email: &str) -> AppResult<()> {
    if let Some(user) = state.db.get_user_by_email(email).await?
        && let Err(error) = create_and_enqueue_token(
            state,
            &user,
            TOKEN_KIND_PASSWORD_RESET,
            KIND_PASSWORD_RESET,
            state.config.password_reset_ttl(),
        )
        .await
    {
        warn!("failed to enqueue password reset email: {error}");
    }
    Ok(())
}

/// Completes a password reset from a single-use token, revoking every refresh
/// token the user holds across all tenants.
pub async fn confirm_password_reset(
    state: &AppState,
    token: &str,
    new_password: &str,
) -> AppResult<()> {
    let record = state
        .db
        .consume_user_token(TOKEN_KIND_PASSWORD_RESET, &hash_token(token))
        .await?;

    let password_hash = state.auth.hash_password(new_password)?;
    let user = state
        .db
        .update_user_password(record.user_id, &password_hash)
        .await?;
    state.db.revoke_all_user_refresh_tokens(user.id).await?;

    audit::record_event(
        state,
        None,
        Some(user.id),
        "user",
        Some(user.id),
        "account.password_reset_completed",
        json!({}),
    )
    .await;

    Ok(())
}

/// Changes the password for an authenticated user. Requires the current
/// password and revokes all existing refresh tokens.
pub async fn change_password(
    state: &AppState,
    user_id: Uuid,
    current_password: &str,
    new_password: &str,
) -> AppResult<()> {
    let user = state.db.get_user_by_id(user_id).await?;
    state
        .auth
        .verify_password(current_password, &user.password_hash)
        .map_err(|_| AppError::Unauthorized("current password is incorrect".into()))?;

    let password_hash = state.auth.hash_password(new_password)?;
    state.db.update_user_password(user.id, &password_hash).await?;
    state.db.revoke_all_user_refresh_tokens(user.id).await?;

    audit::record_event(
        state,
        None,
        Some(user.id),
        "user",
        Some(user.id),
        "account.password_changed",
        json!({}),
    )
    .await;

    Ok(())
}

/// Changes the account email for an authenticated user. Requires the current
/// password; the new address starts unverified and receives a verification
/// email.
pub async fn change_email(
    state: &AppState,
    user_id: Uuid,
    current_password: &str,
    new_email: &str,
) -> AppResult<UserRecord> {
    let user = state.db.get_user_by_id(user_id).await?;
    state
        .auth
        .verify_password(current_password, &user.password_hash)
        .map_err(|_| AppError::Unauthorized("current password is incorrect".into()))?;

    let updated = state.db.update_user_email(user.id, new_email).await?;
    send_verification_email(state, &updated).await;

    audit::record_event(
        state,
        None,
        Some(user.id),
        "user",
        Some(user.id),
        "account.email_changed",
        json!({}),
    )
    .await;

    Ok(updated)
}

async fn create_and_enqueue_token(
    state: &AppState,
    user: &UserRecord,
    token_kind: &str,
    notification_kind: &str,
    ttl: std::time::Duration,
) -> AppResult<()> {
    let token = generate_token();
    let expires_at = Utc::now()
        + ChronoDuration::from_std(ttl)
            .map_err(|error| AppError::internal(format!("invalid token ttl: {error}")))?;

    state
        .db
        .create_user_token(user.id, token_kind, &hash_token(&token), expires_at)
        .await?;

    state
        .db
        .enqueue_notification(
            &NewNotification {
                tenant_id: None,
                user_id: Some(user.id),
                kind: notification_kind.into(),
                recipient: user.email.clone(),
                payload: json!({
                    "token": token,
                    "expires_at": expires_at.to_rfc3339(),
                }),
                dedupe_key: None,
            },
            state.config.max_job_attempts,
        )
        .await?;

    Ok(())
}
