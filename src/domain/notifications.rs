use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

pub const NOTIFICATION_STATUS_PENDING: &str = "pending";
pub const NOTIFICATION_STATUS_SENT: &str = "sent";
pub const NOTIFICATION_STATUS_DEAD_LETTER: &str = "dead_letter";

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct NotificationRecord {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub kind: String,
    pub recipient: String,
    pub payload: Value,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub dedupe_key: Option<String>,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// One entry in the authenticated user's in-app notification feed. Only
/// task-activity kinds are exposed; token-bearing account mails never appear.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationFeedItemResponse {
    pub id: Uuid,
    pub kind: String,
    pub payload: Value,
    pub read_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl From<&NotificationRecord> for NotificationFeedItemResponse {
    fn from(record: &NotificationRecord) -> Self {
        Self {
            id: record.id,
            kind: record.kind.clone(),
            payload: record.payload.clone(),
            read_at: record.read_at,
            created_at: record.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct NotificationFeedResponse {
    pub data: Vec<NotificationFeedItemResponse>,
    pub next_cursor: Option<String>,
    pub unread_count: i64,
}

#[derive(Debug, Clone)]
pub struct NewNotification {
    pub tenant_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    pub kind: String,
    pub recipient: String,
    pub payload: Value,
    pub dedupe_key: Option<String>,
}

/// Per-user delivery switches for optional notification kinds. Security and
/// account mails (verification, password reset, invitations) are always sent.
#[derive(Debug, Clone, Serialize)]
pub struct NotificationPreferencesResponse {
    pub task_due_soon: bool,
    pub task_overdue: bool,
    pub task_commented: bool,
}

impl NotificationPreferencesResponse {
    /// Builds the response from the set of kinds the user has disabled;
    /// anything absent from the list stays enabled by default.
    pub fn from_disabled_kinds(disabled: &[String]) -> Self {
        let is_disabled = |kind: &str| disabled.iter().any(|entry| entry == kind);
        Self {
            task_due_soon: !is_disabled(crate::notify::KIND_TASK_DUE_SOON),
            task_overdue: !is_disabled(crate::notify::KIND_TASK_OVERDUE),
            task_commented: !is_disabled(crate::notify::KIND_TASK_COMMENTED),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct AuditEventRecord {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub actor_user_id: Option<Uuid>,
    pub subject_type: String,
    pub subject_id: Option<Uuid>,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEventResponse {
    pub id: Uuid,
    pub actor_user_id: Option<Uuid>,
    pub subject_type: String,
    pub subject_id: Option<Uuid>,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

impl From<&AuditEventRecord> for AuditEventResponse {
    fn from(value: &AuditEventRecord) -> Self {
        Self {
            id: value.id,
            actor_user_id: value.actor_user_id,
            subject_type: value.subject_type.clone(),
            subject_id: value.subject_id,
            event_type: value.event_type.clone(),
            payload: value.payload.clone(),
            created_at: value.created_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedAuditEvents {
    pub data: Vec<AuditEventResponse>,
    pub next_cursor: Option<String>,
}
