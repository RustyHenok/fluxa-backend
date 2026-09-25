use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::domain::{
    ExportFormat, InvitationResponse, TaskAuditResponse, TaskFilters, TaskResponse,
    TenantMembershipResponse, UserResponse, validate_task_priority, validate_task_status,
};
use crate::error::AppResult;

use super::helpers::{normalize_optional_choice, parse_optional_datetime};

#[derive(Debug, Deserialize)]
pub(super) struct RegisterRequest {
    pub(super) email: String,
    pub(super) password: String,
    pub(super) tenant_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LoginRequest {
    pub(super) email: String,
    pub(super) password: String,
    pub(super) tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(super) struct RefreshRequest {
    pub(super) refresh_token: String,
    pub(super) tenant_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LogoutRequest {
    pub(super) refresh_token: String,
    pub(super) access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct InvitationCreatePayload {
    pub(super) email: String,
    pub(super) role: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct InvitationAcceptPayload {
    pub(super) token: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct MemberRolePayload {
    pub(super) role: String,
}

#[derive(Debug, Serialize)]
pub(super) struct InvitationCreateResponse {
    pub(super) invitation: InvitationResponse,
    pub(super) token: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct SwitchTenantRequest {
    pub(super) tenant_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskPayload {
    pub(super) project_id: Option<Uuid>,
    pub(super) title: String,
    pub(super) description: Option<String>,
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) assignee_id: Option<Uuid>,
    pub(super) due_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct TaskPatchPayload {
    pub(super) project_id: Option<Option<Uuid>>,
    pub(super) title: Option<String>,
    pub(super) description: Option<Option<String>>,
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) assignee_id: Option<Option<Uuid>>,
    pub(super) due_at: Option<Option<DateTime<Utc>>>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub(super) struct TaskListQuery {
    pub(super) limit: Option<usize>,
    pub(super) cursor: Option<String>,
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) project_id: Option<Uuid>,
    pub(super) assignee_id: Option<Uuid>,
    pub(super) label_id: Option<Uuid>,
    pub(super) due_before: Option<String>,
    pub(super) due_after: Option<String>,
    pub(super) updated_after: Option<String>,
    pub(super) q: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub(super) struct ExportRequest {
    pub(super) status: Option<String>,
    pub(super) priority: Option<String>,
    pub(super) project_id: Option<Uuid>,
    pub(super) assignee_id: Option<Uuid>,
    pub(super) label_id: Option<Uuid>,
    pub(super) due_before: Option<String>,
    pub(super) due_after: Option<String>,
    pub(super) updated_after: Option<String>,
    pub(super) q: Option<String>,
    pub(super) format: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct VerifyEmailPayload {
    pub(super) token: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResendVerificationPayload {
    pub(super) email: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PasswordResetRequestPayload {
    pub(super) email: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct PasswordResetConfirmPayload {
    pub(super) token: String,
    pub(super) new_password: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChangePasswordPayload {
    pub(super) current_password: String,
    pub(super) new_password: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ChangeEmailPayload {
    pub(super) current_password: String,
    pub(super) new_email: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub(super) struct AuditListQuery {
    pub(super) limit: Option<usize>,
    pub(super) cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct AuthResponse {
    pub(super) access_token: String,
    pub(super) refresh_token: String,
    pub(super) expires_in_seconds: u64,
    pub(super) user: UserResponse,
    pub(super) active_tenant: TenantMembershipResponse,
}

#[derive(Debug, Serialize)]
pub(super) struct MeResponse {
    pub(super) user: UserResponse,
    pub(super) active_tenant: TenantMembershipResponse,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TaskListResponse {
    pub(super) data: Vec<TaskResponse>,
    pub(super) next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub(super) struct TaskAuditQuery {
    pub(super) limit: Option<usize>,
    pub(super) cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct TaskAuditListResponse {
    pub(super) data: Vec<TaskAuditResponse>,
    pub(super) next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ProjectPayload {
    pub(super) name: String,
    pub(super) description: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct ProjectPatchPayload {
    pub(super) name: Option<String>,
    pub(super) description: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct LabelPayload {
    pub(super) name: String,
    pub(super) color: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(super) struct LabelPatchPayload {
    pub(super) name: Option<String>,
    pub(super) color: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub(super) struct TaskLabelsPayload {
    pub(super) label_ids: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub(super) struct HealthResponse<'a> {
    pub(super) status: &'a str,
}

impl TaskListQuery {
    pub(super) fn into_filters(self) -> AppResult<TaskFilters> {
        TaskFilters {
            status: normalize_optional_choice(self.status)
                .map(|value| validate_task_status(&value))
                .transpose()?,
            priority: normalize_optional_choice(self.priority)
                .map(|value| validate_task_priority(&value))
                .transpose()?,
            project_id: self.project_id,
            assignee_id: self.assignee_id,
            label_id: self.label_id,
            due_before: parse_optional_datetime(self.due_before, "due_before")?,
            due_after: parse_optional_datetime(self.due_after, "due_after")?,
            updated_after: parse_optional_datetime(self.updated_after, "updated_after")?,
            q: self.q.filter(|value| !value.trim().is_empty()),
        }
        .validate()
    }
}

impl ExportRequest {
    pub(super) fn export_format(&self) -> AppResult<ExportFormat> {
        match normalize_optional_choice(self.format.clone()) {
            Some(value) => value.parse(),
            None => Ok(ExportFormat::default()),
        }
    }

    pub(super) fn into_filters(self) -> AppResult<TaskFilters> {
        TaskFilters {
            status: normalize_optional_choice(self.status)
                .map(|value| validate_task_status(&value))
                .transpose()?,
            priority: normalize_optional_choice(self.priority)
                .map(|value| validate_task_priority(&value))
                .transpose()?,
            project_id: self.project_id,
            assignee_id: self.assignee_id,
            label_id: self.label_id,
            due_before: parse_optional_datetime(self.due_before, "due_before")?,
            due_after: parse_optional_datetime(self.due_after, "due_after")?,
            updated_after: parse_optional_datetime(self.updated_after, "updated_after")?,
            q: self.q.filter(|value| !value.trim().is_empty()),
        }
        .validate()
    }
}
