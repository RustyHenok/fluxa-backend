use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const ROLE_OWNER: &str = "owner";
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_MEMBER: &str = "member";

pub fn validate_role(value: &str) -> AppResult<&str> {
    match value {
        ROLE_OWNER | ROLE_ADMIN | ROLE_MEMBER => Ok(value),
        _ => Err(AppError::Validation(format!("unsupported role: {value}"))),
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
