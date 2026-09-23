use chrono::{DateTime, Utc};
use uuid::Uuid;

use super::Database;
use crate::domain::UserTokenRecord;
use crate::error::{AppError, AppResult};

const USER_TOKEN_COLUMNS: &str = "id, user_id, kind, token_hash, expires_at, used_at, created_at";

impl Database {
    /// Stores a new single-use token hash, invalidating any still-pending
    /// tokens of the same kind for the user.
    pub async fn create_user_token(
        &self,
        user_id: Uuid,
        kind: &str,
        token_hash: &str,
        expires_at: DateTime<Utc>,
    ) -> AppResult<UserTokenRecord> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            UPDATE user_tokens
            SET used_at = now()
            WHERE user_id = $1 AND kind = $2 AND used_at IS NULL
            "#,
        )
        .bind(user_id)
        .bind(kind)
        .execute(&mut *tx)
        .await?;

        let record = sqlx::query_as::<_, UserTokenRecord>(&format!(
            r#"
            INSERT INTO user_tokens (id, user_id, kind, token_hash, expires_at, created_at)
            VALUES ($1, $2, $3, $4, $5, now())
            RETURNING {USER_TOKEN_COLUMNS}
            "#
        ))
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(kind)
        .bind(token_hash)
        .bind(expires_at)
        .fetch_one(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(record)
    }

    /// Consumes a token atomically: it must match the hash and kind, be
    /// unused, and be unexpired. Returns the consumed record.
    pub async fn consume_user_token(
        &self,
        kind: &str,
        token_hash: &str,
    ) -> AppResult<UserTokenRecord> {
        sqlx::query_as::<_, UserTokenRecord>(&format!(
            r#"
            UPDATE user_tokens
            SET used_at = now()
            WHERE kind = $1 AND token_hash = $2 AND used_at IS NULL AND expires_at > now()
            RETURNING {USER_TOKEN_COLUMNS}
            "#
        ))
        .bind(kind)
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::Unauthorized("token is invalid or expired".into()))
    }
}
