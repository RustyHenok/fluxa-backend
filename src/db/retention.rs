use super::Database;
use crate::error::AppResult;

impl Database {
    /// Deletes refresh tokens that expired or were revoked longer ago than the
    /// retention window. Active tokens are never touched.
    pub async fn purge_stale_refresh_tokens(&self, retention_days: i64) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM refresh_tokens
            WHERE (expires_at < now() - make_interval(days => $1::int))
               OR (revoked_at IS NOT NULL AND revoked_at < now() - make_interval(days => $1::int))
            "#,
        )
        .bind(retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Deletes terminal (completed or dead-letter) background jobs older than
    /// the retention window, freeing inline payload bloat.
    pub async fn purge_terminal_jobs(&self, retention_days: i64) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM background_jobs
            WHERE status IN ('completed', 'dead_letter')
              AND coalesce(finished_at, scheduled_at) < now() - make_interval(days => $1::int)
            "#,
        )
        .bind(retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Deletes terminal (sent or dead-letter) notifications older than the
    /// retention window.
    pub async fn purge_terminal_notifications(&self, retention_days: i64) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM notifications
            WHERE status IN ('sent', 'dead_letter')
              AND created_at < now() - make_interval(days => $1::int)
            "#,
        )
        .bind(retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Deletes audit log rows older than the retention window.
    pub async fn purge_old_audit_events(&self, retention_days: i64) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            DELETE FROM audit_log
            WHERE created_at < now() - make_interval(days => $1::int)
            "#,
        )
        .bind(retention_days)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }

    /// Counts jobs currently queued, used by the metrics sampler.
    pub async fn count_queued_jobs(&self) -> AppResult<i64> {
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM background_jobs WHERE status = 'queued'")
                .fetch_one(&self.pool)
                .await?;
        Ok(count)
    }
}
