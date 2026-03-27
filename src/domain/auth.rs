use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const ROLE_OWNER: &str = "owner";
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_MEMBER: &str = "member";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MembershipRole {
    Owner,
    Admin,
    Member,
}

impl MembershipRole {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Owner => ROLE_OWNER,
            Self::Admin => ROLE_ADMIN,
            Self::Member => ROLE_MEMBER,
        }
    }
}

impl fmt::Display for MembershipRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for MembershipRole {
    type Err = AppError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            ROLE_OWNER => Ok(Self::Owner),
            ROLE_ADMIN => Ok(Self::Admin),
            ROLE_MEMBER => Ok(Self::Member),
            _ => Err(AppError::Validation(format!("unsupported role: {value}"))),
        }
    }
}

pub fn validate_role(value: &str) -> AppResult<MembershipRole> {
    value.parse()
}

impl MembershipRecord {
    pub fn parsed_role(&self) -> AppResult<MembershipRole> {
        self.role.parse()
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
    pub role: MembershipRole,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<&MembershipRecord> for TenantMembershipResponse {
    type Error = AppError;

    fn try_from(value: &MembershipRecord) -> Result<Self, Self::Error> {
        Ok(Self {
            tenant_id: value.tenant_id,
            tenant_name: value.tenant_name.clone(),
            role: value.parsed_role()?,
            created_at: value.created_at,
        })
    }
}
