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

    pub async fn get_tenant_member(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
    ) -> AppResult<Option<TenantMemberRecord>> {
        sqlx::query_as::<_, TenantMemberRecord>(
            r#"
            SELECT tm.user_id,
                   u.email,
                   tm.role,
                   tm.created_at AS joined_at
            FROM tenant_memberships tm
            JOIN users u ON u.id = tm.user_id
            WHERE tm.tenant_id = $1 AND tm.user_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn update_membership_role(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        role: &str,
    ) -> AppResult<Option<TenantMemberRecord>> {
        sqlx::query_as::<_, TenantMemberRecord>(
            r#"
            UPDATE tenant_memberships tm
            SET role = $3
            FROM users u
            WHERE tm.tenant_id = $1
              AND tm.user_id = $2
              AND u.id = tm.user_id
            RETURNING tm.user_id,
                      u.email,
                      tm.role,
                      tm.created_at AS joined_at
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(role)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn delete_membership(&self, tenant_id: Uuid, user_id: Uuid) -> AppResult<bool> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM tenant_memberships
            WHERE tenant_id = $1 AND user_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .execute(&self.pool)
        .await?;

        Ok(deleted.rows_affected() > 0)
    }

    pub async fn count_tenant_owners(&self, tenant_id: Uuid) -> AppResult<i64> {
        sqlx::query_scalar(
            r#"
            SELECT COUNT(*)
            FROM tenant_memberships
            WHERE tenant_id = $1 AND role = 'owner'
            "#,
        )
        .bind(tenant_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }
}
