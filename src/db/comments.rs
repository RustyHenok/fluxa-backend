use chrono::Utc;
use serde_json::json;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use super::Database;
use crate::domain::{CommentRecord, PaginatedComments};
use crate::error::{AppError, AppResult};
use crate::pagination::AuditCursor;

const COMMENT_COLUMNS: &str = "id, task_id, tenant_id, author_id, body, created_at, updated_at";

impl Database {
    pub async fn create_comment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        author_id: Uuid,
        body: String,
    ) -> AppResult<CommentRecord> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;

        let comment = sqlx::query_as::<_, CommentRecord>(&format!(
            r#"
            INSERT INTO task_comments (id, task_id, tenant_id, author_id, body, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $6, $6)
            RETURNING {COMMENT_COLUMNS}
            "#,
        ))
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(author_id)
        .bind(body)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_comment_added', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(author_id)
        .bind(json!({ "comment_id": comment.id }))
        .bind(now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(comment)
    }

    pub async fn list_task_comments(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        cursor: Option<&AuditCursor>,
        limit: usize,
    ) -> AppResult<PaginatedComments> {
        let mut builder = QueryBuilder::<Postgres>::new(format!(
            r#"
            SELECT {COMMENT_COLUMNS}
            FROM task_comments
            WHERE tenant_id = "#,
        ));
        builder.push_bind(tenant_id);
        builder.push(" AND task_id = ");
        builder.push_bind(task_id);

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

        let mut comments = builder
            .build_query_as::<CommentRecord>()
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if comments.len() > limit {
            comments.truncate(limit);
            comments.last().map(|comment| AuditCursor {
                created_at: comment.created_at,
                id: comment.id,
            })
        } else {
            None
        };

        Ok(PaginatedComments {
            comments,
            next_cursor,
        })
    }

    pub async fn get_comment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        comment_id: Uuid,
    ) -> AppResult<CommentRecord> {
        sqlx::query_as::<_, CommentRecord>(&format!(
            r#"
            SELECT {COMMENT_COLUMNS}
            FROM task_comments
            WHERE id = $1 AND tenant_id = $2 AND task_id = $3
            "#,
        ))
        .bind(comment_id)
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("comment not found".into()))
    }

    pub async fn update_comment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        comment_id: Uuid,
        body: String,
    ) -> AppResult<CommentRecord> {
        sqlx::query_as::<_, CommentRecord>(&format!(
            r#"
            UPDATE task_comments
            SET body = $4, updated_at = now()
            WHERE id = $1 AND tenant_id = $2 AND task_id = $3
            RETURNING {COMMENT_COLUMNS}
            "#,
        ))
        .bind(comment_id)
        .bind(tenant_id)
        .bind(task_id)
        .bind(body)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("comment not found".into()))
    }

    pub async fn delete_comment(
        &self,
        tenant_id: Uuid,
        task_id: Uuid,
        comment_id: Uuid,
        actor_id: Uuid,
    ) -> AppResult<()> {
        let mut tx = self.pool.begin().await?;

        let deleted = sqlx::query_scalar::<_, Uuid>(
            r#"
            DELETE FROM task_comments
            WHERE id = $1 AND tenant_id = $2 AND task_id = $3
            RETURNING id
            "#,
        )
        .bind(comment_id)
        .bind(tenant_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;

        let Some(comment_id) = deleted else {
            return Err(AppError::NotFound("comment not found".into()));
        };

        sqlx::query(
            r#"
            INSERT INTO task_audit_log (id, task_id, tenant_id, actor_user_id, event_type, payload, created_at)
            VALUES ($1, $2, $3, $4, 'task_comment_deleted', $5, $6)
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(task_id)
        .bind(tenant_id)
        .bind(actor_id)
        .bind(json!({ "comment_id": comment_id }))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }
}
