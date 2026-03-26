use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::Database;
use crate::domain::RefreshTokenRecord;
use crate::error::{AppError, AppResult};

impl Database {
    pub async fn create_refresh_token(
        &self,
        token_id: Uuid,
        user_id: Uuid,
        tenant_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (id, user_id, tenant_id, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(token_id)
        .bind(user_id)
        .bind(tenant_id)
        .bind(expires_at)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_refresh_token(&self, token_id: Uuid) -> AppResult<Option<RefreshTokenRecord>> {
        sqlx::query_as::<_, RefreshTokenRecord>(
            r#"
            SELECT id, user_id, tenant_id, expires_at, revoked_at, replaced_by, created_at
            FROM refresh_tokens
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn rotate_refresh_token(
        &self,
        previous_token_id: Uuid,
        next_token_id: Uuid,
        user_id: Uuid,
        tenant_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;
        let now = Utc::now();

        let updated = sqlx::query(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = $2, replaced_by = $3
            WHERE id = $1 AND revoked_at IS NULL AND expires_at > $2
            "#,
        )
        .bind(previous_token_id)
        .bind(now)
        .bind(next_token_id)
        .execute(&mut *tx)
        .await?;

        if updated.rows_affected() == 0 {
            return Err(AppError::Unauthorized(
                "refresh token is expired or already revoked".into(),
            ));
        }

        sqlx::query(
            r#"
            INSERT INTO refresh_tokens (id, user_id, tenant_id, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5)
            "#,
        )
        .bind(next_token_id)
        .bind(user_id)
        .bind(tenant_id)
        .bind(expires_at)
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn revoke_refresh_token(&self, token_id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE refresh_tokens
            SET revoked_at = COALESCE(revoked_at, $2)
            WHERE id = $1
            "#,
        )
        .bind(token_id)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
