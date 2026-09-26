use chrono::{Duration as ChronoDuration, Utc};
use uuid::Uuid;

use super::Database;
use crate::domain::{
    WEBHOOK_DELIVERY_STATUS_DEAD_LETTER, WEBHOOK_DELIVERY_STATUS_PENDING, WebhookDeliveryRecord,
    WebhookRecord,
};
use crate::error::{AppError, AppResult};
use crate::pagination::AuditCursor;

const WEBHOOK_COLUMNS: &str =
    "id, tenant_id, url, secret, events, is_active, created_by, created_at, updated_at";

const WEBHOOK_DELIVERY_COLUMNS: &str = "id, webhook_id, tenant_id, event_type, payload, status, \
     attempts, max_attempts, scheduled_at, delivered_at, last_error, created_at";

pub struct PaginatedWebhookDeliveries {
    pub deliveries: Vec<WebhookDeliveryRecord>,
    pub next_cursor: Option<AuditCursor>,
}

impl Database {
    pub async fn create_webhook(
        &self,
        tenant_id: Uuid,
        actor_id: Uuid,
        url: &str,
        secret: &str,
        events: &[String],
    ) -> AppResult<WebhookRecord> {
        let now = Utc::now();
        sqlx::query_as::<_, WebhookRecord>(&format!(
            r#"
            INSERT INTO webhooks (id, tenant_id, url, secret, events, is_active, created_by, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, TRUE, $6, $7, $7)
            RETURNING {WEBHOOK_COLUMNS}
            "#,
        ))
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(url)
        .bind(secret)
        .bind(events)
        .bind(actor_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_webhooks(&self, tenant_id: Uuid) -> AppResult<Vec<WebhookRecord>> {
        sqlx::query_as::<_, WebhookRecord>(&format!(
            r#"
            SELECT {WEBHOOK_COLUMNS}
            FROM webhooks
            WHERE tenant_id = $1
            ORDER BY created_at DESC, id DESC
            "#,
        ))
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn count_webhooks(&self, tenant_id: Uuid) -> AppResult<i64> {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM webhooks WHERE tenant_id = $1")
            .bind(tenant_id)
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::from)
    }

    pub async fn get_webhook(&self, tenant_id: Uuid, webhook_id: Uuid) -> AppResult<WebhookRecord> {
        sqlx::query_as::<_, WebhookRecord>(&format!(
            r#"
            SELECT {WEBHOOK_COLUMNS}
            FROM webhooks
            WHERE tenant_id = $1 AND id = $2
            "#,
        ))
        .bind(tenant_id)
        .bind(webhook_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("webhook not found".into()))
    }

    pub async fn update_webhook(
        &self,
        tenant_id: Uuid,
        webhook_id: Uuid,
        url: Option<&str>,
        events: Option<&[String]>,
        is_active: Option<bool>,
    ) -> AppResult<WebhookRecord> {
        sqlx::query_as::<_, WebhookRecord>(&format!(
            r#"
            UPDATE webhooks
            SET url = COALESCE($3, url),
                events = COALESCE($4, events),
                is_active = COALESCE($5, is_active),
                updated_at = $6
            WHERE tenant_id = $1 AND id = $2
            RETURNING {WEBHOOK_COLUMNS}
            "#,
        ))
        .bind(tenant_id)
        .bind(webhook_id)
        .bind(url)
        .bind(events)
        .bind(is_active)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("webhook not found".into()))
    }

    pub async fn delete_webhook(&self, tenant_id: Uuid, webhook_id: Uuid) -> AppResult<()> {
        let result = sqlx::query("DELETE FROM webhooks WHERE tenant_id = $1 AND id = $2")
            .bind(tenant_id)
            .bind(webhook_id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(AppError::NotFound("webhook not found".into()));
        }
        Ok(())
    }

    /// Fans an event out to every active webhook in the tenant subscribed to
    /// the event type, creating one pending delivery row per webhook.
    pub async fn enqueue_webhook_deliveries(
        &self,
        tenant_id: Uuid,
        event_type: &str,
        payload: &serde_json::Value,
    ) -> AppResult<u64> {
        let result = sqlx::query(
            r#"
            INSERT INTO webhook_deliveries (id, webhook_id, tenant_id, event_type, payload, status, scheduled_at, created_at)
            SELECT gen_random_uuid(), id, tenant_id, $2, $3, 'pending', $4, $4
            FROM webhooks
            WHERE tenant_id = $1 AND is_active AND $2 = ANY(events)
            "#,
        )
        .bind(tenant_id)
        .bind(event_type)
        .bind(payload)
        .bind(Utc::now())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// Claims a batch of due pending deliveries, bumping their attempt count
    /// and pushing `scheduled_at` forward so concurrent workers skip them.
    pub async fn claim_pending_webhook_deliveries(
        &self,
        limit: i64,
    ) -> AppResult<Vec<WebhookDeliveryRecord>> {
        sqlx::query_as::<_, WebhookDeliveryRecord>(&format!(
            r#"
            UPDATE webhook_deliveries
            SET attempts = attempts + 1,
                scheduled_at = now() + interval '60 seconds'
            WHERE id IN (
                SELECT id FROM webhook_deliveries
                WHERE status = $1 AND scheduled_at <= now()
                ORDER BY scheduled_at ASC
                LIMIT $2
                FOR UPDATE SKIP LOCKED
            )
            RETURNING {WEBHOOK_DELIVERY_COLUMNS}
            "#,
        ))
        .bind(WEBHOOK_DELIVERY_STATUS_PENDING)
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn mark_webhook_delivery_delivered(&self, delivery_id: Uuid) -> AppResult<()> {
        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET status = 'delivered', delivered_at = now(), last_error = NULL
            WHERE id = $1
            "#,
        )
        .bind(delivery_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fail_webhook_delivery(
        &self,
        delivery: &WebhookDeliveryRecord,
        message: &str,
    ) -> AppResult<()> {
        let status = if delivery.attempts >= delivery.max_attempts {
            WEBHOOK_DELIVERY_STATUS_DEAD_LETTER
        } else {
            WEBHOOK_DELIVERY_STATUS_PENDING
        };

        let next_time = if status == WEBHOOK_DELIVERY_STATUS_DEAD_LETTER {
            Utc::now()
        } else {
            let seconds = 2_i64.pow(delivery.attempts.clamp(1, 8) as u32);
            Utc::now() + ChronoDuration::seconds(seconds)
        };

        sqlx::query(
            r#"
            UPDATE webhook_deliveries
            SET status = $2, scheduled_at = $3, last_error = $4
            WHERE id = $1
            "#,
        )
        .bind(delivery.id)
        .bind(status)
        .bind(next_time)
        .bind(message)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Loads the signing secret for a delivery's webhook, if it still exists.
    pub async fn get_webhook_secret(&self, webhook_id: Uuid) -> AppResult<Option<String>> {
        sqlx::query_scalar::<_, String>("SELECT secret FROM webhooks WHERE id = $1")
            .bind(webhook_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(AppError::from)
    }

    pub async fn list_webhook_deliveries(
        &self,
        tenant_id: Uuid,
        webhook_id: Uuid,
        cursor: Option<&AuditCursor>,
        limit: usize,
    ) -> AppResult<PaginatedWebhookDeliveries> {
        let mut builder = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
            "SELECT {WEBHOOK_DELIVERY_COLUMNS} FROM webhook_deliveries WHERE tenant_id = "
        ));
        builder.push_bind(tenant_id);
        builder.push(" AND webhook_id = ");
        builder.push_bind(webhook_id);

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

        let mut deliveries = builder
            .build_query_as::<WebhookDeliveryRecord>()
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if deliveries.len() > limit {
            deliveries.truncate(limit);
            deliveries.last().map(|delivery| AuditCursor {
                created_at: delivery.created_at,
                id: delivery.id,
            })
        } else {
            None
        };

        Ok(PaginatedWebhookDeliveries {
            deliveries,
            next_cursor,
        })
    }
}
