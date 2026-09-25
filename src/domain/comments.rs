use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::pagination::AuditCursor;

pub const MAX_COMMENT_BODY_LENGTH: usize = 4000;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CommentRecord {
    pub id: Uuid,
    pub task_id: Uuid,
    pub tenant_id: Uuid,
    pub author_id: Uuid,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentResponse {
    pub id: Uuid,
    pub task_id: Uuid,
    pub author_id: Uuid,
    pub body: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&CommentRecord> for CommentResponse {
    fn from(value: &CommentRecord) -> Self {
        Self {
            id: value.id,
            task_id: value.task_id,
            author_id: value.author_id,
            body: value.body.clone(),
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedComments {
    pub comments: Vec<CommentRecord>,
    pub next_cursor: Option<AuditCursor>,
}

pub fn validate_comment_body(body: &str) -> AppResult<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation(
            "comment body must not be empty".into(),
        ));
    }
    if trimmed.chars().count() > MAX_COMMENT_BODY_LENGTH {
        return Err(AppError::Validation(format!(
            "comment body must be at most {MAX_COMMENT_BODY_LENGTH} characters"
        )));
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::{MAX_COMMENT_BODY_LENGTH, validate_comment_body};

    #[test]
    fn comment_body_is_trimmed_and_bounded() {
        assert_eq!(
            validate_comment_body("  hello world  ").expect("valid body"),
            "hello world"
        );
        assert!(validate_comment_body("   ").is_err());
        assert!(validate_comment_body(&"x".repeat(MAX_COMMENT_BODY_LENGTH + 1)).is_err());
        assert!(validate_comment_body(&"x".repeat(MAX_COMMENT_BODY_LENGTH)).is_ok());
    }
}
