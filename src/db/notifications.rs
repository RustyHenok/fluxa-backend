use chrono::{Duration as ChronoDuration, Utc};
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use super::Database;
use crate::domain::{
    NOTIFICATION_STATUS_DEAD_LETTER, NOTIFICATION_STATUS_PENDING, NewNotification,
    NotificationRecord,
};
use crate::error::AppResult;
use crate::pagination::AuditCursor;

const NOTIFICATION_COLUMNS: &str = "id, tenant_id, user_id, kind, recipient, payload, status, \
     attempts, max_attempts, scheduled_at, sent_at, last_error, dedupe_key, read_at, created_at";

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

    /// Lists the notification kinds this user has explicitly disabled.
    pub async fn list_disabled_notification_kinds(&self, user_id: Uuid) -> AppResult<Vec<String>> {
        sqlx::query_scalar(
            r#"
            SELECT kind
            FROM notification_preferences
            WHERE user_id = $1 AND NOT enabled
            ORDER BY kind
            "#,
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(crate::error::AppError::from)
    }

    /// Upserts a delivery switch for one notification kind.
    pub async fn set_notification_preference(
        &self,
        user_id: Uuid,
        kind: &str,
        enabled: bool,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO notification_preferences (user_id, kind, enabled, updated_at)
            VALUES ($1, $2, $3, now())
            ON CONFLICT (user_id, kind)
            DO UPDATE SET enabled = EXCLUDED.enabled, updated_at = now()
            "#,
        )
        .bind(user_id)
        .bind(kind)
        .bind(enabled)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Returns whether the user still accepts the given notification kind.
    /// Absence of a preference row means the kind is enabled.
    pub async fn is_notification_kind_enabled(&self, user_id: Uuid, kind: &str) -> AppResult<bool> {
        let enabled: Option<bool> = sqlx::query_scalar(
            r#"
            SELECT enabled
            FROM notification_preferences
            WHERE user_id = $1 AND kind = $2
            "#,
        )
        .bind(user_id)
        .bind(kind)
        .fetch_optional(&self.pool)
        .await?;
        Ok(enabled.unwrap_or(true))
    }

    /// Lists the user's in-app notification feed for one tenant, restricted to
    /// the given kinds, newest first with keyset pagination.
    pub async fn list_notification_feed(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        kinds: &[&str],
        unread_only: bool,
        cursor: Option<&AuditCursor>,
        limit: usize,
    ) -> AppResult<(Vec<NotificationRecord>, Option<AuditCursor>)> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            "SELECT {NOTIFICATION_COLUMNS} FROM notifications WHERE tenant_id = "
        ));
        builder.push_bind(tenant_id);
        builder.push(" AND user_id = ");
        builder.push_bind(user_id);
        builder.push(" AND kind = ANY(");
        builder.push_bind(
            kinds
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>(),
        );
        builder.push(")");

        if unread_only {
            builder.push(" AND read_at IS NULL");
        }

        if let Some(cursor) = cursor {
            builder.push(" AND (created_at < ");
            builder.push_bind(cursor.created_at);
            builder.push(" OR (created_at = ");
            builder.push_bind(cursor.created_at);
            builder.push(" AND id < ");
            builder.push_bind(cursor.id);
            builder.push("))");
        }

        builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind((limit + 1) as i64);

        let mut entries = builder
            .build_query_as::<NotificationRecord>()
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if entries.len() > limit {
            entries.truncate(limit);
            entries.last().map(|entry| AuditCursor {
                created_at: entry.created_at,
                id: entry.id,
            })
        } else {
            None
        };

        Ok((entries, next_cursor))
    }

    /// Counts the user's unread feed notifications for one tenant.
    pub async fn count_unread_notifications(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        kinds: &[&str],
    ) -> AppResult<i64> {
        sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM notifications
            WHERE tenant_id = $1 AND user_id = $2 AND kind = ANY($3) AND read_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(
            kinds
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>(),
        )
        .fetch_one(&self.pool)
        .await
        .map_err(crate::error::AppError::from)
    }

    /// Marks one of the user's feed notifications as read. Returns `false`
    /// when the id does not match one of the user's feed entries.
    pub async fn mark_notification_read(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        notification_id: Uuid,
        kinds: &[&str],
    ) -> AppResult<bool> {
        let result = sqlx::query(
            r#"
            UPDATE notifications
            SET read_at = COALESCE(read_at, now())
            WHERE id = $1 AND tenant_id = $2 AND user_id = $3 AND kind = ANY($4)
            "#,
        )
        .bind(notification_id)
        .bind(tenant_id)
        .bind(user_id)
        .bind(
            kinds
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>(),
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Marks all of the user's unread feed notifications as read, returning
    /// how many rows changed.
    pub async fn mark_all_notifications_read(
        &self,
        tenant_id: Uuid,
        user_id: Uuid,
        kinds: &[&str],
    ) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            UPDATE notifications
            SET read_at = now()
            WHERE tenant_id = $1 AND user_id = $2 AND kind = ANY($3) AND read_at IS NULL
            "#,
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(
            kinds
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<_>>(),
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}
