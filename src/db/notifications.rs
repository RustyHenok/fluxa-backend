use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use super::Database;
use crate::domain::{
    NOTIFICATION_STATUS_DEAD_LETTER, NOTIFICATION_STATUS_PENDING, NewNotification,
    NotificationRecord,
};
use crate::error::AppResult;

const NOTIFICATION_COLUMNS: &str = "id, tenant_id, user_id, kind, recipient, payload, status, \
     attempts, max_attempts, scheduled_at, sent_at, last_error, dedupe_key, created_at";

impl Database {
    /// Inserts a notification into the outbox. Returns `false` when a
    /// dedupe key collision means an equivalent notification already exists.
    pub async fn enqueue_notification(
        &self,
        notification: &NewNotification,
        max_attempts: i32,
    ) -> AppResult<bool> {
        let result = sqlx::query(
            r#"
            INSERT INTO notifications (
                id, tenant_id, user_id, kind, recipient, payload,
                status, attempts, max_attempts, scheduled_at, dedupe_key, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, 'pending', 0, $7, now(), $8, now())
            ON CONFLICT (dedupe_key) WHERE dedupe_key IS NOT NULL DO NOTHING
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(notification.tenant_id)
        .bind(notification.user_id)
        .bind(&notification.kind)
        .bind(&notification.recipient)
        .bind(&notification.payload)
        .bind(max_attempts)
        .bind(&notification.dedupe_key)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Atomically claims due pending notifications for delivery, bumping the
    /// attempt counter so a crashed worker retries later.
    pub async fn claim_pending_notifications(
        &self,
        limit: i64,
    ) -> AppResult<Vec<NotificationRecord>> {
        sqlx::query_as::<_, NotificationRecord>(&format!(
            r#"
            UPDATE notifications
            SET attempts = attempts + 1,
                scheduled_at = now() + interval '60 seconds'
            WHERE id IN (
                SELECT id FROM notifications
                WHERE status = $1 AND scheduled_at <= now()
                ORDER BY scheduled_at ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            RETURNING {NOTIFICATION_COLUMNS}
            "#
        ))
        .bind(NOTIFICATION_STATUS_PENDING)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(crate::error::AppError::from)
    }

    pub async fn mark_notification_sent(&self, notification_id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE notifications
            SET status = 'sent', sent_at = now(), last_error = NULL
            WHERE id = $1
            "#,
        )
        .bind(notification_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_notification(
        &self,
        notification: &NotificationRecord,
        message: &str,
    ) -> AppResult<()> {
        let status = if notification.attempts >= notification.max_attempts {
            NOTIFICATION_STATUS_DEAD_LETTER
        } else {
            NOTIFICATION_STATUS_PENDING
        };

        let next_time = if status == NOTIFICATION_STATUS_DEAD_LETTER {
            Utc::now()
        } else {
            let seconds = 2_i64.pow(notification.attempts.clamp(1, 8) as u32);
            Utc::now() + ChronoDuration::seconds(seconds)
        };

        sqlx::query(
            r#"
            UPDATE notifications
            SET status = $2, scheduled_at = $3, last_error = $4
            WHERE id = $1
            "#,
        )
        .bind(notification.id)
        .bind(status)
        .bind(next_time)
        .bind(message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
