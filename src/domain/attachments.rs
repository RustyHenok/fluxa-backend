use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const MAX_ATTACHMENT_FILE_NAME_LENGTH: usize = 255;
pub const MAX_ATTACHMENTS_PER_TASK: i64 = 20;
pub const MAX_ATTACHMENT_CONTENT_TYPE_LENGTH: usize = 100;
pub const DEFAULT_ATTACHMENT_CONTENT_TYPE: &str = "application/octet-stream";

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AttachmentRecord {
    pub id: Uuid,
    pub task_id: Uuid,
    pub tenant_id: Uuid,
    pub uploaded_by: Uuid,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub storage_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentResponse {
    pub id: Uuid,
    pub task_id: Uuid,
    pub uploaded_by: Uuid,
    pub file_name: String,
    pub content_type: String,
    pub size_bytes: i64,
    pub download_path: String,
    pub created_at: DateTime<Utc>,
}

impl From<&AttachmentRecord> for AttachmentResponse {
    fn from(value: &AttachmentRecord) -> Self {
        Self {
            id: value.id,
            task_id: value.task_id,
            uploaded_by: value.uploaded_by,
            file_name: value.file_name.clone(),
            content_type: value.content_type.clone(),
            size_bytes: value.size_bytes,
            download_path: format!(
                "/v1/tasks/{}/attachments/{}/download",
                value.task_id, value.id
            ),
            created_at: value.created_at,
        }
    }
}

pub fn validate_attachment_file_name(file_name: &str) -> AppResult<String> {
    let trimmed = file_name.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation("file_name must not be empty".into()));
    }
    if trimmed.chars().count() > MAX_ATTACHMENT_FILE_NAME_LENGTH {
        return Err(AppError::Validation(format!(
            "file_name must be at most {MAX_ATTACHMENT_FILE_NAME_LENGTH} characters"
        )));
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || matches!(c, '/' | '\\' | '"'))
    {
        return Err(AppError::Validation(
            "file_name must not contain path separators, quotes, or control characters".into(),
        ));
    }
    if trimmed == "." || trimmed == ".." {
        return Err(AppError::Validation("file_name is not allowed".into()));
    }
    Ok(trimmed.to_string())
}

pub fn normalize_attachment_content_type(content_type: Option<&str>) -> String {
    let candidate = content_type.map(str::trim).unwrap_or("");
    if candidate.is_empty()
        || candidate.chars().count() > MAX_ATTACHMENT_CONTENT_TYPE_LENGTH
        || !candidate.chars().all(|c| c.is_ascii_graphic() || c == ' ')
    {
        return DEFAULT_ATTACHMENT_CONTENT_TYPE.to_string();
    }
    candidate.to_string()
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_ATTACHMENT_CONTENT_TYPE, MAX_ATTACHMENT_FILE_NAME_LENGTH,
        normalize_attachment_content_type, validate_attachment_file_name,
    };

    #[test]
    fn file_name_validation_rejects_unsafe_names() {
        assert_eq!(
            validate_attachment_file_name("  report.pdf  ").expect("valid name"),
            "report.pdf"
        );
        assert!(validate_attachment_file_name("").is_err());
        assert!(validate_attachment_file_name("   ").is_err());
        assert!(validate_attachment_file_name("a/b.txt").is_err());
        assert!(validate_attachment_file_name("a\\b.txt").is_err());
        assert!(validate_attachment_file_name("a\"b.txt").is_err());
        assert!(validate_attachment_file_name("..").is_err());
        assert!(validate_attachment_file_name("evil\u{0}.txt").is_err());
        assert!(
            validate_attachment_file_name(&"x".repeat(MAX_ATTACHMENT_FILE_NAME_LENGTH + 1))
                .is_err()
        );
    }

    #[test]
    fn content_type_normalization_falls_back_to_octet_stream() {
        assert_eq!(
            normalize_attachment_content_type(Some("text/plain; charset=utf-8")),
            "text/plain; charset=utf-8"
        );
        assert_eq!(
            normalize_attachment_content_type(None),
            DEFAULT_ATTACHMENT_CONTENT_TYPE
        );
        assert_eq!(
            normalize_attachment_content_type(Some("   ")),
            DEFAULT_ATTACHMENT_CONTENT_TYPE
        );
        assert_eq!(
            normalize_attachment_content_type(Some("bad\u{7}type")),
            DEFAULT_ATTACHMENT_CONTENT_TYPE
        );
        assert_eq!(
            normalize_attachment_content_type(Some(&"x".repeat(200))),
            DEFAULT_ATTACHMENT_CONTENT_TYPE
        );
    }
}
