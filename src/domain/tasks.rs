use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;

pub const TASK_STATUS_OPEN: &str = "open";
pub const TASK_STATUS_IN_PROGRESS: &str = "in_progress";
pub const TASK_STATUS_DONE: &str = "done";
pub const TASK_STATUS_ARCHIVED: &str = "archived";

pub const TASK_PRIORITY_LOW: &str = "low";
pub const TASK_PRIORITY_MEDIUM: &str = "medium";
pub const TASK_PRIORITY_HIGH: &str = "high";
pub const TASK_PRIORITY_URGENT: &str = "urgent";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Done,
    Archived,
}

impl TaskStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => TASK_STATUS_OPEN,
            Self::InProgress => TASK_STATUS_IN_PROGRESS,
            Self::Done => TASK_STATUS_DONE,
            Self::Archived => TASK_STATUS_ARCHIVED,
        }
    }
}

impl fmt::Display for TaskStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TaskStatus {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            TASK_STATUS_OPEN => Ok(Self::Open),
            TASK_STATUS_IN_PROGRESS => Ok(Self::InProgress),
            TASK_STATUS_DONE => Ok(Self::Done),
            TASK_STATUS_ARCHIVED => Ok(Self::Archived),
            _ => Err(AppError::Validation(format!(
                "unsupported task status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Urgent,
}

impl TaskPriority {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => TASK_PRIORITY_LOW,
            Self::Medium => TASK_PRIORITY_MEDIUM,
            Self::High => TASK_PRIORITY_HIGH,
            Self::Urgent => TASK_PRIORITY_URGENT,
        }
    }
}

impl fmt::Display for TaskPriority {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for TaskPriority {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            TASK_PRIORITY_LOW => Ok(Self::Low),
            TASK_PRIORITY_MEDIUM => Ok(Self::Medium),
            TASK_PRIORITY_HIGH => Ok(Self::High),
            TASK_PRIORITY_URGENT => Ok(Self::Urgent),
            _ => Err(AppError::Validation(format!(
                "unsupported task priority: {value}"
            ))),
        }
    }
}

pub fn validate_task_status(value: &str) -> AppResult<TaskStatus> {
    value.parse()
}

pub fn validate_task_priority(value: &str) -> AppResult<TaskPriority> {
    value.parse()
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

impl TaskRecord {
    pub fn parsed_status(&self) -> AppResult<TaskStatus> {
        self.status.parse()
    }

    pub fn parsed_priority(&self) -> AppResult<TaskPriority> {
        self.priority.parse()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFilters {
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub assignee_id: Option<Uuid>,
    pub due_before: Option<DateTime<Utc>>,
    pub due_after: Option<DateTime<Utc>>,
    pub updated_after: Option<DateTime<Utc>>,
    pub q: Option<String>,
}

impl TaskFilters {
    pub fn validate(self) -> AppResult<Self> {
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
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
}

impl CreateTaskInput {
    pub fn validate(self) -> AppResult<Self> {
        if self.title.trim().is_empty() {
            return Err(AppError::Validation("title is required".into()));
        }

        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateTaskInput {
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub status: Option<TaskStatus>,
    pub priority: Option<TaskPriority>,
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
        Ok(self)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedTasks {
    pub tasks: Vec<TaskRecord>,
    pub next_cursor: Option<Cursor>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct DashboardSummary {
    pub open_task_count: i64,
    pub in_progress_task_count: i64,
    pub done_task_count: i64,
    pub overdue_task_count: i64,
    pub recent_activity_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub assignee_id: Option<Uuid>,
    pub due_at: Option<DateTime<Utc>>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl TryFrom<&TaskRecord> for TaskResponse {
    type Error = AppError;

    fn try_from(value: &TaskRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            tenant_id: value.tenant_id,
            title: value.title.clone(),
            description: value.description.clone(),
            status: value.parsed_status()?,
            priority: value.parsed_priority()?,
            assignee_id: value.assignee_id,
            due_at: value.due_at,
            created_by: value.created_by,
            updated_by: value.updated_by,
            created_at: value.created_at,
            updated_at: value.updated_at,
        })
    }
}
