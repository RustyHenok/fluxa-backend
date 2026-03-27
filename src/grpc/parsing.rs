use chrono::{DateTime, Utc};
use tonic::Status;
use uuid::Uuid;

use crate::domain::TaskFilters;
use crate::error::AppError;

pub(super) fn filters_from_parts(
    status: String,
    priority: String,
    assignee_id: Option<Uuid>,
    due_before: Option<DateTime<Utc>>,
    due_after: Option<DateTime<Utc>>,
    updated_after: Option<DateTime<Utc>>,
    q: Option<String>,
) -> Result<TaskFilters, Status> {
    TaskFilters {
        status: option_string(status).map(|value| value.to_ascii_lowercase()),
        priority: option_string(priority).map(|value| value.to_ascii_lowercase()),
        assignee_id,
        due_before,
        due_after,
        updated_after,
        q,
    }
    .validate()
    .map_err(status_from_error)
}

pub(super) fn parse_uuid(value: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value)
        .map_err(|error| Status::invalid_argument(format!("invalid {field}: {error}")))
}

pub(super) fn option_uuid(value: String) -> Result<Option<Uuid>, Status> {
    let value = option_string(value);
    value.map(|value| parse_uuid(&value, "uuid")).transpose()
}

pub(super) fn option_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

pub(super) fn option_datetime(value: String) -> Result<Option<DateTime<Utc>>, Status> {
    let value = match option_string(value) {
        Some(value) => value,
        None => return Ok(None),
    };

    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| Status::invalid_argument(format!("invalid datetime: {error}")))
}

pub(super) fn status_from_error(error: AppError) -> Status {
    match error {
        AppError::Validation(message) => Status::invalid_argument(message),
        AppError::Unauthorized(message) => Status::unauthenticated(message),
        AppError::Forbidden(message) => Status::permission_denied(message),
        AppError::NotFound(message) => Status::not_found(message),
        AppError::Conflict(message) => Status::already_exists(message),
        AppError::RateLimited(message) => Status::resource_exhausted(message),
        AppError::Internal(message) => Status::internal(message),
    }
}
