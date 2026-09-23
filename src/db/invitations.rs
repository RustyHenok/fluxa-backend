use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::Database;
use crate::domain::{InvitationRecord, MembershipRecord};
use crate::error::{AppError, AppResult};

impl Database {
    pub async fn create_invitation(
        &self,
        tenant_id: Uuid,
        email: &str,
        role: &str,
        token_hash: &str,
        expires_at: DateTime<Utc>,
        created_by: Uuid,
    ) -> AppResult<InvitationRecord> {
        sqlx::query_as::<_, InvitationRecord>(
            r#"
            INSERT INTO tenant_invitations (
                id, tenant_id, email, role, token_hash, expires_at, created_by, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            RETURNING id, tenant_id, email, role, token_hash, expires_at, accepted_at,
                      revoked_at, created_by, created_at
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(email)
        .bind(role)
        .bind(token_hash)
        .bind(expires_at)
        .bind(created_by)
        .bind(Utc::now())
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_pending_invitations(
        &self,
        tenant_id: Uuid,
    ) -> AppResult<Vec<InvitationRecord>> {
        sqlx::query_as::<_, InvitationRecord>(
            r#"
            SELECT id, tenant_id, email, role, token_hash, expires_at, accepted_at,
                   revoked_at, created_by, created_at
            FROM tenant_invitations
            WHERE tenant_id = $1
              AND accepted_at IS NULL
              AND revoked_at IS NULL
              AND expires_at > now()
            ORDER BY created_at DESC, id DESC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn revoke_invitation(&self, tenant_id: Uuid, invitation_id: Uuid) -> AppResult<()> {
        let updated = sqlx::query(
            r#"
            UPDATE tenant_invitations
            SET revoked_at = now()
            WHERE id = $1
              AND tenant_id = $2
              AND accepted_at IS NULL
              AND revoked_at IS NULL
            "#,
        )
        .bind(invitation_id)
        .bind(tenant_id)
        .execute(&self.pool)
        .await?;

        if updated.rows_affected() == 0 {
            return Err(AppError::NotFound("invitation not found".into()));
        }

        Ok(())
    }

    pub async fn accept_invitation(
        &self,
        tenant_id: Uuid,
        token_hash: &str,
        user_id: Uuid,
        user_email: &str,
    ) -> AppResult<MembershipRecord> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();

        let invitation = sqlx::query_as::<_, InvitationRecord>(
            r#"
            UPDATE tenant_invitations
            SET accepted_at = $3
            WHERE tenant_id = $1
              AND token_hash = $2
              AND accepted_at IS NULL
              AND revoked_at IS NULL
              AND expires_at > $3
            RETURNING id, tenant_id, email, role, token_hash, expires_at, accepted_at,
                      revoked_at, created_by, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::NotFound("invitation is invalid, expired, or revoked".into()))?;

        if !invitation.email.eq_ignore_ascii_case(user_email) {
            return Err(AppError::Forbidden(
                "invitation was issued for a different email address".into(),
            ));
        }

        let membership = sqlx::query_as::<_, MembershipRecord>(
            r#"
            INSERT INTO tenant_memberships (tenant_id, user_id, role, created_at)
            VALUES ($1, $2, $3, $4)
            RETURNING tenant_id,
                      (SELECT name FROM tenants WHERE id = $1) AS tenant_name,
                      user_id,
                      role,
                      created_at
            "#,
        )
        .bind(invitation.tenant_id)
        .bind(user_id)
        .bind(&invitation.role)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
        .map_err(AppError::from)?;

        tx.commit().await?;
        Ok(membership)
    }
}
