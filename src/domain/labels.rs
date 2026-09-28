use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const MAX_LABEL_NAME_LENGTH: usize = 64;
pub const MAX_LABELS_PER_TASK: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct LabelRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateLabelInput {
    pub name: String,
    pub color: Option<String>,
}

impl CreateLabelInput {
    pub fn validate(mut self) -> AppResult<Self> {
        self.name = validate_label_name(&self.name)?;
        self.color = self
            .color
            .map(|color| validate_label_color(&color))
            .transpose()?;
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateLabelInput {
    pub name: Option<String>,
    pub color: Option<Option<String>>,
}

impl UpdateLabelInput {
    pub fn validate(mut self) -> AppResult<Self> {
        if self.name.is_none() && self.color.is_none() {
            return Err(AppError::Validation(
                "at least one label field must be provided".into(),
            ));
        }

        self.name = self
            .name
            .map(|name| validate_label_name(&name))
            .transpose()?;
        self.color = self
            .color
            .map(|color| color.map(|value| validate_label_color(&value)).transpose())
            .transpose()?;
        Ok(self)
    }
}

fn validate_label_name(name: &str) -> AppResult<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation("label name is required".into()));
    }
    if trimmed.chars().count() > MAX_LABEL_NAME_LENGTH {
        return Err(AppError::Validation(format!(
            "label name must be at most {MAX_LABEL_NAME_LENGTH} characters"
        )));
    }
    Ok(trimmed.to_string())
}

/// Accepts `#RRGGBB` hex colors and normalizes them to lowercase.
pub fn validate_label_color(color: &str) -> AppResult<String> {
    let trimmed = color.trim();
    let is_valid = trimmed.len() == 7
        && trimmed.starts_with('#')
        && trimmed[1..].chars().all(|c| c.is_ascii_hexdigit());
    if !is_valid {
        return Err(AppError::Validation(
            "label color must be a hex color like #4f46e5".into(),
        ));
    }
    Ok(trimmed.to_ascii_lowercase())
}

pub fn validate_task_label_ids(label_ids: &[Uuid]) -> AppResult<Vec<Uuid>> {
    if label_ids.len() > MAX_LABELS_PER_TASK {
        return Err(AppError::Validation(format!(
            "a task can have at most {MAX_LABELS_PER_TASK} labels"
        )));
    }
    let mut unique = label_ids.to_vec();
    unique.sort();
    unique.dedup();
    Ok(unique)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub color: Option<String>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&LabelRecord> for LabelResponse {
    fn from(value: &LabelRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            name: value.name.clone(),
            color: value.color.clone(),
            created_by: value.created_by,
            updated_by: value.updated_by,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_label_color, validate_task_label_ids};
    use uuid::Uuid;

    #[test]
    fn color_validation_accepts_hex_and_normalizes_case() {
        assert_eq!(validate_label_color(" #4F46E5 ").unwrap(), "#4f46e5");
        assert!(validate_label_color("#123").is_err());
        assert!(validate_label_color("4f46e5").is_err());
        assert!(validate_label_color("#zzzzzz").is_err());
    }

    #[test]
    fn task_label_ids_are_deduplicated() {
        let id = Uuid::new_v4();
        let unique = validate_task_label_ids(&[id, id]).unwrap();
        assert_eq!(unique.len(), 1);
    }
}
