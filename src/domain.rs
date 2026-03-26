use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;

pub const ROLE_OWNER: &str = "owner";
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_MEMBER: &str = "member";

pub const TASK_STATUS_OPEN: &str = "open";
pub const TASK_STATUS_IN_PROGRESS: &str = "in_progress";
pub const TASK_STATUS_DONE: &str = "done";
pub const TASK_STATUS_ARCHIVED: &str = "archived";

pub const TASK_PRIORITY_LOW: &str = "low";
pub const TASK_PRIORITY_MEDIUM: &str = "medium";
pub const TASK_PRIORITY_HIGH: &str = "high";
pub const TASK_PRIORITY_URGENT: &str = "urgent";

pub const JOB_STATUS_QUEUED: &str = "queued";
pub const JOB_STATUS_RUNNING: &str = "running";
pub const JOB_STATUS_COMPLETED: &str = "completed";
pub const JOB_STATUS_DEAD_LETTER: &str = "dead_letter";

pub const JOB_TYPE_TASK_EXPORT: &str = "task_export";
pub const JOB_TYPE_DUE_REMINDER_SWEEP: &str = "due_reminder_sweep";

pub fn validate_role(value: &str) -> AppResult<&str> {
    match value {
        ROLE_OWNER | ROLE_ADMIN | ROLE_MEMBER => Ok(value),
        _ => Err(AppError::Validation(format!("unsupported role: {value}"))),
    }
}

pub fn validate_task_status(value: &str) -> AppResult<&str> {
    match value {
        TASK_STATUS_OPEN | TASK_STATUS_IN_PROGRESS | TASK_STATUS_DONE | TASK_STATUS_ARCHIVED => {
            Ok(value)
        }
        _ => Err(AppError::Validation(format!(
            "unsupported task status: {value}"
        ))),
    }
}

pub fn validate_task_priority(value: &str) -> AppResult<&str> {
    match value {
        TASK_PRIORITY_LOW | TASK_PRIORITY_MEDIUM | TASK_PRIORITY_HIGH | TASK_PRIORITY_URGENT => {
            Ok(value)
        }
        _ => Err(AppError::Validation(format!(
            "unsupported task priority: {value}"
        ))),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct UserRecord {
    pub id: Uuid,
    pub email: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TenantRecord {
    pub id: Uuid,
    pub name: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct MembershipRecord {
    pub tenant_id: Uuid,
    pub tenant_name: String,
    pub user_id: Uuid,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct RefreshTokenRecord {
    pub id: Uuid,
    pub user_id: Uuid,
    pub tenant_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub replaced_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TaskRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct BackgroundJobRecord {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub job_type: String,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub payload: Value,
    pub result_payload: Option<Value>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFilters {
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub due_before: Option<DateTime<Utc>>,
    pub due_after: Option<DateTime<Utc>>,
    pub updated_after: Option<DateTime<Utc>>,
    pub q: Option<String>,
}

impl TaskFilters {
    pub fn validate(self) -> AppResult<Self> {
        if let Some(status) = &self.status {
            validate_task_status(status)?;
        }

        if let Some(priority) = &self.priority {
            validate_task_priority(priority)?;
        }

        Ok(self)
    }

    pub fn export_payload(&self) -> Value {
        serde_json::json!({
            "status": self.status,
            "priority": self.priority,
            "assignee_id": self.assignee_id,
            "due_before": self.due_before,
            "due_after": self.due_after,
            "updated_after": self.updated_after,
            "q": self.q,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTaskInput {
    pub title: String,
    pub description: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
}

impl CreateTaskInput {
    pub fn validate(self) -> AppResult<Self> {
        if self.title.trim().is_empty() {
            return Err(AppError::Validation("title is required".into()));
        }

        if let Some(status) = &self.status {
            validate_task_status(status)?;
        }

        if let Some(priority) = &self.priority {
            validate_task_priority(priority)?;
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateTaskInput {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub assignee_id: Option<Option<Uuid>>,
    pub due_at: Option<Option<DateTime<Utc>>>,
}

impl UpdateTaskInput {
    pub fn validate(self) -> AppResult<Self> {
        if let Some(title) = &self.title {
            if title.trim().is_empty() {
                return Err(AppError::Validation("title cannot be empty".into()));
            }
        }

        if let Some(status) = &self.status {
            validate_task_status(status)?;
        }

        if let Some(priority) = &self.priority {
            validate_task_priority(priority)?;
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedTasks {
    pub tasks: Vec<TaskRecord>,
    pub next_cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub created_at: DateTime<Utc>,
}

impl From<&UserRecord> for UserResponse {
    fn from(value: &UserRecord) -> Self {
        Self {
            id: value.id,
            email: value.email.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantMembershipResponse {
    pub tenant_id: Uuid,
    pub tenant_name: String,
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl From<&MembershipRecord> for TenantMembershipResponse {
    fn from(value: &MembershipRecord) -> Self {
        Self {
            tenant_id: value.tenant_id,
            tenant_name: value.tenant_name.clone(),
            role: value.role.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&TaskRecord> for TaskResponse {
    fn from(value: &TaskRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            title: value.title.clone(),
            description: value.description.clone(),
            status: value.status.clone(),
            priority: value.priority.clone(),
            assignee_id: value.assignee_id,
            due_at: value.due_at,
            created_by: value.created_by,
            updated_by: value.updated_by,
            created_at: value.created_at,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub job_type: String,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub payload: Value,
    pub result_payload: Option<Value>,
    pub last_error: Option<String>,
}

impl From<&BackgroundJobRecord> for JobResponse {
    fn from(value: &BackgroundJobRecord) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            job_type: value.job_type.clone(),
            status: value.status.clone(),
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            scheduled_at: value.scheduled_at,
            started_at: value.started_at,
            finished_at: value.finished_at,
            payload: value.payload.clone(),
            result_payload: value.result_payload.clone(),
            last_error: value.last_error.clone(),
        }
    }
}
