use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProjectInput {
    pub name: String,
    pub description: Option<String>,
}

impl CreateProjectInput {
    pub fn validate(self) -> AppResult<Self> {
        if self.name.trim().is_empty() {
            return Err(AppError::Validation("project name is required".into()));
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateProjectInput {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
}

impl UpdateProjectInput {
    pub fn validate(self) -> AppResult<Self> {
        if let Some(name) = &self.name
            && name.trim().is_empty()
        {
            return Err(AppError::Validation("project name cannot be empty".into()));
        }

        if self.name.is_none() && self.description.is_none() {
            return Err(AppError::Validation(
                "at least one project field must be provided".into(),
            ));
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&ProjectRecord> for ProjectResponse {
    fn from(value: &ProjectRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            name: value.name.clone(),
            description: value.description.clone(),
            created_by: value.created_by,
            updated_by: value.updated_by,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct ProjectSummary {
    pub project_id: Uuid,
    pub project_name: String,
    pub open_task_count: i64,
    pub in_progress_task_count: i64,
    pub done_task_count: i64,
    pub overdue_task_count: i64,
    pub recent_activity_count: i64,
}
