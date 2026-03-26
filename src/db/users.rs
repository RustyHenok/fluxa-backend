use chrono::Utc;
use uuid::Uuid;

use super::Database;
use crate::domain::{MembershipRecord, TenantRecord, UserRecord};
use crate::error::{AppError, AppResult};

impl Database {
    pub async fn create_user_with_tenant(
        &self,
        email: &str,
        password_hash: &str,
        tenant_name: &str,
    ) -> AppResult<(UserRecord, MembershipRecord)> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();
        let user_id = Uuid::new_v4();
        let tenant_id = Uuid::new_v4();

        let user = sqlx::query_as::<_, UserRecord>(
            r#"
            INSERT INTO users (id, email, password_hash, created_at)
            VALUES ($1, $2, $3, $4)
            RETURNING id, email, password_hash, created_at
            "#,
        )
        .bind(user_id)
        .bind(email)
        .bind(password_hash)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        let tenant = sqlx::query_as::<_, TenantRecord>(
            r#"
            INSERT INTO tenants (id, name, created_at)
            VALUES ($1, $2, $3)
            RETURNING id, name, created_at
            "#,
        )
        .bind(tenant_id)
        .bind(tenant_name)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        let membership = sqlx::query_as::<_, MembershipRecord>(
            r#"
            INSERT INTO tenant_memberships (tenant_id, user_id, role, created_at)
            VALUES ($1, $2, 'owner', $3)
            RETURNING tenant_id,
                      $4 AS tenant_name,
                      user_id,
                      role,
                      created_at
            "#,
        )
        .bind(tenant.id)
        .bind(user.id)
        .bind(now)
        .bind(tenant.name.clone())
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok((user, membership))
    }

    pub async fn get_user_by_email(&self, email: &str) -> AppResult<Option<UserRecord>> {
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, email, password_hash, created_at
            FROM users
            WHERE email = $1
            "#,
        )
        .bind(email)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_user_by_id(&self, user_id: Uuid) -> AppResult<UserRecord> {
        sqlx::query_as::<_, UserRecord>(
            r#"
            SELECT id, email, password_hash, created_at
            FROM users
            WHERE id = $1
            "#,
        )
        .bind(user_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("user not found".into()))
    }
}
