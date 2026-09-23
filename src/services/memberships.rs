use chrono::{Duration as ChronoDuration, Utc};
use serde_json::json;
use uuid::Uuid;

use crate::domain::{InvitationRecord, MembershipRecord, MembershipRole, TenantMemberRecord};
use crate::error::{AppError, AppResult};
use crate::notify::KIND_TENANT_INVITATION;
use crate::services::audit;
use crate::state::AppState;
use crate::tokens::{generate_token, hash_token};

#[derive(Debug, Clone)]
pub struct CreatedInvitation {
    pub invitation: InvitationRecord,
    pub token: String,
}

/// Creates an invitation for `email` to join `tenant_id` with `role`.
///
/// Only owners and admins may invite; granting the `admin` role requires the
/// `owner` role. The plaintext token is returned exactly once so the inviter
/// can distribute it until mailer support lands.
pub async fn create_invitation(
    state: &AppState,
    tenant_id: Uuid,
    actor_role: MembershipRole,
    actor_user_id: Uuid,
    email: &str,
    role: MembershipRole,
) -> AppResult<CreatedInvitation> {
    match role {
        MembershipRole::Owner => {
            return Err(AppError::Validation(
                "owners cannot be invited; promote an existing member instead".into(),
            ));
        }
        MembershipRole::Admin => {
            if actor_role != MembershipRole::Owner {
                return Err(AppError::Forbidden(
                    "owner role required to invite an admin".into(),
                ));
            }
        }
        MembershipRole::Member => {}
    }

    if let Some(existing_user) = state.db.get_user_by_email(email).await?
        && state
            .db
            .get_membership(existing_user.id, tenant_id)
            .await?
            .is_some()
    {
        return Err(AppError::Conflict(
            "user is already a member of this tenant".into(),
        ));
    }

    let token = generate_token();
    let expires_at = Utc::now()
        + ChronoDuration::from_std(state.config.invitation_ttl())
            .map_err(|error| AppError::internal(format!("invalid invitation ttl: {error}")))?;

    let invitation = state
        .db
        .create_invitation(
            tenant_id,
            email,
            role.as_str(),
            &hash_token(&token),
            expires_at,
            actor_user_id,
        )
        .await
        .map_err(|error| match error {
            AppError::Conflict(_) => {
                AppError::Conflict("a pending invitation already exists for this email".into())
            }
            other => other,
        })?;

    let enqueued = state
        .db
        .enqueue_notification(
            &crate::domain::NewNotification {
                tenant_id: Some(tenant_id),
                user_id: None,
                kind: KIND_TENANT_INVITATION.into(),
                recipient: email.to_owned(),
                payload: json!({
                    "role": role.as_str(),
                    "token": token,
                    "expires_at": expires_at.to_rfc3339(),
                }),
                dedupe_key: None,
            },
            state.config.max_job_attempts,
        )
        .await;
    if let Err(error) = enqueued {
        tracing::warn!("failed to enqueue invitation notification: {error}");
    }

    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_user_id),
        "invitation",
        Some(invitation.id),
        "invitation.created",
        json!({ "email": email, "role": role.as_str() }),
    )
    .await;

    Ok(CreatedInvitation { invitation, token })
}

pub async fn list_invitations(
    state: &AppState,
    tenant_id: Uuid,
) -> AppResult<Vec<InvitationRecord>> {
    state.db.list_pending_invitations(tenant_id).await
}

pub async fn revoke_invitation(
    state: &AppState,
    tenant_id: Uuid,
    actor_user_id: Uuid,
    invitation_id: Uuid,
) -> AppResult<()> {
    state.db.revoke_invitation(tenant_id, invitation_id).await?;
    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_user_id),
        "invitation",
        Some(invitation_id),
        "invitation.revoked",
        json!({}),
    )
    .await;
    Ok(())
}

/// Accepts an invitation token on behalf of the calling user. The invitation
/// must target the caller's email address, be unexpired, and be unused.
pub async fn accept_invitation(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    token: &str,
) -> AppResult<MembershipRecord> {
    let user = state.db.get_user_by_id(user_id).await?;
    let membership = state
        .db
        .accept_invitation(tenant_id, &hash_token(token), user.id, &user.email)
        .await?;

    audit::record_event(
        state,
        Some(tenant_id),
        Some(user_id),
        "membership",
        Some(user_id),
        "invitation.accepted",
        json!({ "role": membership.role }),
    )
    .await;

    Ok(membership)
}

/// Updates a member's role. Only owners may grant or revoke the `owner` and
/// `admin` roles, and the last owner of a tenant can never be demoted.
pub async fn update_member_role(
    state: &AppState,
    tenant_id: Uuid,
    actor_role: MembershipRole,
    actor_user_id: Uuid,
    target_user_id: Uuid,
    new_role: MembershipRole,
) -> AppResult<TenantMemberRecord> {
    let target = state
        .db
        .get_tenant_member(tenant_id, target_user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("member not found".into()))?;
    let current_role: MembershipRole = target.role.parse()?;

    let touches_privileged_role =
        matches!(current_role, MembershipRole::Owner | MembershipRole::Admin)
            || matches!(new_role, MembershipRole::Owner | MembershipRole::Admin);
    if touches_privileged_role && actor_role != MembershipRole::Owner {
        return Err(AppError::Forbidden(
            "owner role required to change owner or admin roles".into(),
        ));
    }

    if current_role == MembershipRole::Owner
        && new_role != MembershipRole::Owner
        && state.db.count_tenant_owners(tenant_id).await? <= 1
    {
        return Err(AppError::Conflict(
            "a tenant must retain at least one owner".into(),
        ));
    }

    let updated = state
        .db
        .update_membership_role(tenant_id, target_user_id, new_role.as_str())
        .await?
        .ok_or_else(|| AppError::NotFound("member not found".into()))?;

    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_user_id),
        "membership",
        Some(target_user_id),
        "member.role_updated",
        json!({ "from": current_role.as_str(), "to": new_role.as_str() }),
    )
    .await;

    Ok(updated)
}

/// Removes a member from the tenant, revoking the member's refresh tokens for
/// this tenant so revoked members cannot mint new access tokens.
pub async fn remove_member(
    state: &AppState,
    tenant_id: Uuid,
    actor_role: MembershipRole,
    actor_user_id: Uuid,
    target_user_id: Uuid,
) -> AppResult<()> {
    let target = state
        .db
        .get_tenant_member(tenant_id, target_user_id)
        .await?
        .ok_or_else(|| AppError::NotFound("member not found".into()))?;
    let target_role: MembershipRole = target.role.parse()?;

    if matches!(target_role, MembershipRole::Owner | MembershipRole::Admin)
        && actor_role != MembershipRole::Owner
    {
        return Err(AppError::Forbidden(
            "owner role required to remove an owner or admin".into(),
        ));
    }

    if target_role == MembershipRole::Owner && state.db.count_tenant_owners(tenant_id).await? <= 1 {
        return Err(AppError::Conflict(
            "a tenant must retain at least one owner".into(),
        ));
    }

    if !state
        .db
        .delete_membership(tenant_id, target_user_id)
        .await?
    {
        return Err(AppError::NotFound("member not found".into()));
    }

    state
        .db
        .revoke_user_tenant_refresh_tokens(target_user_id, tenant_id)
        .await?;

    audit::record_event(
        state,
        Some(tenant_id),
        Some(actor_user_id),
        "membership",
        Some(target_user_id),
        "member.removed",
        json!({ "role": target_role.as_str() }),
    )
    .await;

    Ok(())
}
