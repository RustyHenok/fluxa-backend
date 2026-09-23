use serde_json::Value;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use super::Database;
use crate::error::AppResult;
use crate::pagination::AuditCursor;

use crate::domain::AuditEventRecord;

impl Database {
    /// Records an audit event. Audit writes are best-effort from the caller's
    /// perspective; failures should be logged, never abort the business
    /// operation that already succeeded.
    pub async fn record_audit_event(
        &self,
        tenant_id: Option<Uuid>,
        actor_user_id: Option<Uuid>,
        subject_type: &str,
        subject_id: Option<Uuid>,
        event_type: &str,
        payload: Value,
    ) -> AppResult<()> {
        sqlx::query(
            r#"
            INSERT INTO audit_log (
                id, tenant_id, actor_user_id, subject_type, subject_id,
                event_type, payload, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, now())
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(actor_user_id)
        .bind(subject_type)
        .bind(subject_id)
        .bind(event_type)
        .bind(payload)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_audit_events(
        &self,
        tenant_id: Uuid,
        cursor: Option<&AuditCursor>,
        limit: usize,
    ) -> AppResult<(Vec<AuditEventRecord>, Option<AuditCursor>)> {
        let mut builder = QueryBuilder::<Postgres>::new(
            r#"
            SELECT id, tenant_id, actor_user_id, subject_type, subject_id,
                   event_type, payload, created_at
            FROM audit_log
            WHERE tenant_id = "#,
        );
        builder.push_bind(tenant_id);

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
            .build_query_as::<AuditEventRecord>()
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
}
