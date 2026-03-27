use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const JOB_STATUS_QUEUED: &str = "queued";
pub const JOB_STATUS_RUNNING: &str = "running";
pub const JOB_STATUS_COMPLETED: &str = "completed";
pub const JOB_STATUS_DEAD_LETTER: &str = "dead_letter";

pub const JOB_TYPE_TASK_EXPORT: &str = "task_export";
pub const JOB_TYPE_DUE_REMINDER_SWEEP: &str = "due_reminder_sweep";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    DeadLetter,
}

impl JobStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => JOB_STATUS_QUEUED,
            Self::Running => JOB_STATUS_RUNNING,
            Self::Completed => JOB_STATUS_COMPLETED,
            Self::DeadLetter => JOB_STATUS_DEAD_LETTER,
        }
    }
}

impl fmt::Display for JobStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for JobStatus {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            JOB_STATUS_QUEUED => Ok(Self::Queued),
            JOB_STATUS_RUNNING => Ok(Self::Running),
            JOB_STATUS_COMPLETED => Ok(Self::Completed),
            JOB_STATUS_DEAD_LETTER => Ok(Self::DeadLetter),
            _ => Err(AppError::Validation(format!(
                "unsupported job status: {value}"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobType {
    TaskExport,
    DueReminderSweep,
}

impl JobType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskExport => JOB_TYPE_TASK_EXPORT,
            Self::DueReminderSweep => JOB_TYPE_DUE_REMINDER_SWEEP,
        }
    }
}

impl fmt::Display for JobType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for JobType {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            JOB_TYPE_TASK_EXPORT => Ok(Self::TaskExport),
            JOB_TYPE_DUE_REMINDER_SWEEP => Ok(Self::DueReminderSweep),
            _ => Err(AppError::Validation(format!(
                "unsupported job type: {value}"
            ))),
        }
    }
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

impl BackgroundJobRecord {
    pub fn parsed_status(&self) -> AppResult<JobStatus> {
        self.status.parse()
    }

    pub fn parsed_job_type(&self) -> AppResult<JobType> {
        self.job_type.parse()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub job_type: JobType,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    pub payload: Value,
    pub result_payload: Option<Value>,
    pub last_error: Option<String>,
}

impl TryFrom<&BackgroundJobRecord> for JobResponse {
    type Error = AppError;

    fn try_from(value: &BackgroundJobRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            tenant_id: value.tenant_id,
            job_type: value.parsed_job_type()?,
            status: value.parsed_status()?,
            attempts: value.attempts,
            max_attempts: value.max_attempts,
            scheduled_at: value.scheduled_at,
            started_at: value.started_at,
            finished_at: value.finished_at,
            payload: value.payload.clone(),
            result_payload: value.result_payload.clone(),
            last_error: value.last_error.clone(),
        })
    }
}
