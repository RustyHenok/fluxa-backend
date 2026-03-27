use uuid::Uuid;

use super::Database;
use crate::domain::{MembershipRecord, TenantMemberRecord};
use crate::error::{AppError, AppResult};

impl Database {
    pub async fn list_memberships(&self, user_id: Uuid) -> AppResult<Vec<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1
            ORDER BY tm.created_at ASC
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_membership(
        &self,
        user_id: Uuid,
        tenant_id: Uuid,
    ) -> AppResult<Option<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1 AND tm.tenant_id = $2
            "#,
        )
        .bind(user_id)
        .bind(tenant_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_default_membership(
        &self,
        user_id: Uuid,
    ) -> AppResult<Option<MembershipRecord>> {
        sqlx::query_as::<_, MembershipRecord>(
            r#"
            SELECT tm.tenant_id,
                   t.name AS tenant_name,
                   tm.user_id,
                   tm.role,
                   tm.created_at
            FROM tenant_memberships tm
            JOIN tenants t ON t.id = tm.tenant_id
            WHERE tm.user_id = $1
            ORDER BY tm.created_at ASC
            LIMIT 1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_tenant_members(&self, tenant_id: Uuid) -> AppResult<Vec<TenantMemberRecord>> {
        sqlx::query_as::<_, TenantMemberRecord>(
            r#"
            SELECT tm.user_id,
                   u.email,
                   tm.role,
                   tm.created_at AS joined_at
            FROM tenant_memberships tm
            JOIN users u ON u.id = tm.user_id
            WHERE tm.tenant_id = $1
            ORDER BY tm.created_at ASC, u.email ASC
            "#,
        )
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }
}
