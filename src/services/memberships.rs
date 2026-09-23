use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::domain::{InvitationRecord, MembershipRecord, MembershipRole, TenantMemberRecord};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

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

    let token = generate_invitation_token();
    let expires_at = Utc::now()
        + ChronoDuration::from_std(state.config.invitation_ttl())
            .map_err(|error| AppError::internal(format!("invalid invitation ttl: {error}")))?;

    let invitation = state
        .db
        .create_invitation(
            tenant_id,
            email,
            role.as_str(),
            &hash_invitation_token(&token),
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
    invitation_id: Uuid,
) -> AppResult<()> {
    state.db.revoke_invitation(tenant_id, invitation_id).await
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
    state
        .db
        .accept_invitation(
            tenant_id,
            &hash_invitation_token(token),
            user.id,
            &user.email,
        )
        .await
}

/// Updates a member's role. Only owners may grant or revoke the `owner` and
/// `admin` roles, and the last owner of a tenant can never be demoted.
pub async fn update_member_role(
    state: &AppState,
    tenant_id: Uuid,
    actor_role: MembershipRole,
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

    state
        .db
        .update_membership_role(tenant_id, target_user_id, new_role.as_str())
        .await?
        .ok_or_else(|| AppError::NotFound("member not found".into()))
}

/// Removes a member from the tenant, revoking the member's refresh tokens for
/// this tenant so revoked members cannot mint new access tokens.
pub async fn remove_member(
    state: &AppState,
    tenant_id: Uuid,
    actor_role: MembershipRole,
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
        .await
}

fn generate_invitation_token() -> String {
    let mut bytes = [0u8; 32];
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn hash_invitation_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::{generate_invitation_token, hash_invitation_token};

    #[test]
    fn tokens_are_unique_and_hash_deterministically() {
        let first = generate_invitation_token();
        let second = generate_invitation_token();
        assert_ne!(first, second);
        assert_eq!(hash_invitation_token(&first), hash_invitation_token(&first));
        assert_ne!(
            hash_invitation_token(&first),
            hash_invitation_token(&second)
        );
    }
}
