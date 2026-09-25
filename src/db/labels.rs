use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use super::Database;
use crate::domain::{CreateLabelInput, LabelRecord, UpdateLabelInput};
use crate::error::{AppError, AppResult};

const LABEL_COLUMNS: &str =
    "id, tenant_id, name, color, created_by, updated_by, created_at, updated_at";

impl Database {
    pub async fn create_label(
        &self,
        tenant_id: Uuid,
        actor_id: Uuid,
        input: CreateLabelInput,
    ) -> AppResult<LabelRecord> {
        let now = Utc::now();
        sqlx::query_as::<_, LabelRecord>(&format!(
            r#"
            INSERT INTO labels (id, tenant_id, name, color, created_by, updated_by, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $5, $6, $6)
            RETURNING {LABEL_COLUMNS}
            "#,
        ))
        .bind(Uuid::new_v4())
        .bind(tenant_id)
        .bind(input.name)
        .bind(input.color)
        .bind(actor_id)
        .bind(now)
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn list_labels(&self, tenant_id: Uuid) -> AppResult<Vec<LabelRecord>> {
        sqlx::query_as::<_, LabelRecord>(&format!(
            r#"
            SELECT {LABEL_COLUMNS}
            FROM labels
            WHERE tenant_id = $1
            ORDER BY LOWER(name) ASC, id ASC
            "#,
        ))
        .bind(tenant_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    pub async fn update_label(
        &self,
        tenant_id: Uuid,
        label_id: Uuid,
        actor_id: Uuid,
        input: UpdateLabelInput,
    ) -> AppResult<LabelRecord> {
        let color_provided = input.color.is_some();
        let color_value = input.color.flatten();
        sqlx::query_as::<_, LabelRecord>(&format!(
            r#"
            UPDATE labels
            SET name = COALESCE($3, name),
                color = CASE WHEN $4 THEN $5 ELSE color END,
                updated_by = $6,
                updated_at = $7
            WHERE tenant_id = $1 AND id = $2
            RETURNING {LABEL_COLUMNS}
            "#,
        ))
        .bind(tenant_id)
        .bind(label_id)
        .bind(input.name)
        .bind(color_provided)
        .bind(color_value)
        .bind(actor_id)
        .bind(Utc::now())
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("label not found".into()))
    }

    pub async fn delete_label(&self, tenant_id: Uuid, label_id: Uuid) -> AppResult<LabelRecord> {
        sqlx::query_as::<_, LabelRecord>(&format!(
            r#"
            DELETE FROM labels
            WHERE tenant_id = $1 AND id = $2
            RETURNING {LABEL_COLUMNS}
            "#,
        ))
        .bind(tenant_id)
        .bind(label_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("label not found".into()))
    }

    pub async fn list_task_labels(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
    ) -> AppResult<Vec<LabelRecord>> {
        sqlx::query_as::<_, LabelRecord>(
            r#"
            SELECT l.id, l.tenant_id, l.name, l.color, l.created_by, l.updated_by, l.created_at, l.updated_at
            FROM labels l
            JOIN task_labels tl ON tl.label_id = l.id
            WHERE l.tenant_id = $1 AND tl.task_id = $2
            ORDER BY LOWER(l.name) ASC, l.id ASC
            "#,
        )
        .bind(tenant_id)
        .bind(task_id)
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)
    }

    /// Replaces the full label set on a task and records the change in the
    /// task audit log. All labels must belong to the task's tenant.
    pub async fn set_task_labels(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        actor_id: Uuid,
        label_ids: &[Uuid],
    ) -> AppResult<Vec<LabelRecord>> {
        let mut tx = self.pool.begin().await?;

        let labels = sqlx::query_as::<_, LabelRecord>(&format!(
            r#"
            SELECT {LABEL_COLUMNS}
            FROM labels
            WHERE tenant_id = $1 AND id = ANY($2)
            ORDER BY LOWER(name) ASC, id ASC
            "#,
        ))
        .bind(tenant_id)
        .bind(label_ids)
        .fetch_all(&mut *tx)
        .await?;

        if labels.len() != label_ids.len() {
            return Err(AppError::NotFound("one or more labels not found".into()));
        }

        sqlx::query("DELETE FROM task_labels WHERE task_id = $1 AND label_id <> ALL($2)")
            .bind(task_id)
            .bind(label_ids)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            INSERT INTO task_labels (task_id, label_id)
            SELECT $1, UNNEST($2::uuid[])
            ON CONFLICT DO NOTHING
            "#,
        )
        .bind(task_id)
        .bind(label_ids)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_labels_updated', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(actor_id)
        .bind(json!({
            "label_ids": label_ids,
            "labels": labels.iter().map(|label| label.name.clone()).collect::<Vec<_>>(),
        }))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(labels)
    }
}
