use chrono::Utc;
use uuid::Uuid;

use super::Database;
use crate::domain::OAuthAccountRecord;
use crate::error::{AppError, AppResult};

impl Database {
    /// Returns the user linked to the given provider identity, if any.
    pub async fn get_oauth_account_user(
        &self,
        provider: &str,
        provider_subject: &str,
    ) -> AppResult<Option<Uuid>> {
        sqlx::query_scalar::<_, Uuid>(
            "SELECT user_id FROM oauth_accounts WHERE provider = $1 AND provider_subject = $2",
        )
        .bind(provider)
        .bind(provider_subject)
        .fetch_optional(&self.pool)
        .await
        .map_err(AppError::from)
    }

    /// Links a provider identity to a user. Fails with a conflict if the
    /// identity is already linked (for example by a concurrent login).
    pub async fn link_oauth_account(
        &self,
        user_id: Uuid,
        provider: &str,
        provider_subject: &str,
    ) -> AppResult<()> {
        let result = sqlx::query(
            r#"
            INSERT INTO oauth_accounts (id, user_id, provider, provider_subject, created_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (provider, provider_subject) DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(provider)
        .bind(provider_subject)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(AppError::Conflict(
                "this provider account is already linked".into(),
            ));
        }
        Ok(())
    }

    /// Lists the provider identities linked to a user.
    pub async fn list_oauth_accounts(&self, user_id: Uuid) -> AppResult<Vec<OAuthAccountRecord>> {
        sqlx::query_as::<_, OAuthAccountRecord>(
            "SELECT provider, created_at FROM oauth_accounts WHERE user_id = $1 ORDER BY provider",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    /// Removes a user's link to the given provider. Returns whether a link
    /// existed.
    pub async fn unlink_oauth_account(&self, user_id: Uuid, provider: &str) -> AppResult<bool> {
        let result = sqlx::query("DELETE FROM oauth_accounts WHERE user_id = $1 AND provider = $2")
            .bind(user_id)
            .bind(provider)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}
