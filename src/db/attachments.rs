use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::Database;
use crate::domain::AttachmentRecord;
use crate::error::{AppError, AppResult};

const ATTACHMENT_COLUMNS: &str = "id, task_id, tenant_id, uploaded_by, file_name, content_type, size_bytes, storage_key, created_at";

impl Database {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_attachment(
        &self,
        attachment_id: Uuid,
        tenant_id: Uuid,
        task_id: Uuid,
        uploaded_by: Uuid,
        file_name: &str,
        content_type: &str,
        size_bytes: i64,
        storage_key: &str,
    ) -> AppResult<AttachmentRecord> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;

        let attachment = sqlx::query_as::<_, AttachmentRecord>(&format!(
            r#"
            INSERT INTO task_attachments (
                id, task_id, tenant_id, uploaded_by, file_name, content_type,
                size_bytes, storage_key, created_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            RETURNING {ATTACHMENT_COLUMNS}
            "#,
        ))
        .bind(attachment_id)
        .bind(task_id)
        .bind(tenant_id)
        .bind(uploaded_by)
        .bind(file_name)
        .bind(content_type)
        .bind(size_bytes)
        .bind(storage_key)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_attachment_added', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(uploaded_by)
        .bind(json!({
            "attachment_id": attachment.id,
            "file_name": attachment.file_name,
            "size_bytes": attachment.size_bytes,
        }))
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(attachment)
    }

    pub async fn list_task_attachments(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<Vec<AttachmentRecord>> {
        sqlx::query_as::<_, AttachmentRecord>(&format!(
            r#"
            SELECT {ATTACHMENT_COLUMNS}
            FROM task_attachments
            WHERE tenant_id = $1 AND task_id = $2
            ORDER BY created_at DESC, id DESC
            "#,
        ))
        .bind(tenant_id)
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn count_task_attachments(&self, tenant_id: Uuid, task_id: Uuid) -> AppResult<i64> {
        sqlx::query_scalar::<_, i64>(
            r#"
            SELECT COUNT(*)::BIGINT
            FROM task_attachments
            WHERE tenant_id = $1 AND task_id = $2
            "#,
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn get_attachment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        attachment_id: Uuid,
    ) -> AppResult<AttachmentRecord> {
        sqlx::query_as::<_, AttachmentRecord>(&format!(
            r#"
            SELECT {ATTACHMENT_COLUMNS}
            FROM task_attachments
            WHERE id = $1 AND tenant_id = $2 AND task_id = $3
            "#,
        ))
        .bind(attachment_id)
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("attachment not found".into()))
    }

    pub async fn delete_attachment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        attachment_id: Uuid,
        actor_id: Uuid,
    ) -> AppResult<AttachmentRecord> {
        let mut tx = self.pool.begin().await?;

        let attachment = sqlx::query_as::<_, AttachmentRecord>(&format!(
            r#"
            DELETE FROM task_attachments
            WHERE id = $1 AND tenant_id = $2 AND task_id = $3
            RETURNING {ATTACHMENT_COLUMNS}
            "#,
        ))
        .bind(attachment_id)
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::NotFound("attachment not found".into()))?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_attachment_deleted', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(actor_id)
        .bind(json!({
            "attachment_id": attachment.id,
            "file_name": attachment.file_name,
        }))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(attachment)
    }
}
