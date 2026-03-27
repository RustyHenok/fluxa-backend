use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cursor {
    pub updated_at: DateTime<Utc>,
    pub id: Uuid,
}

impl Cursor {
    pub fn encode(&self) -> AppResult<String> {
        let json = serde_json::to_vec(self)
            .map_err(|error| AppError::internal(format!("failed to encode cursor: {error}")))?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(value: &str) -> AppResult<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|error| AppError::Validation(format!("invalid cursor encoding: {error}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| AppError::Validation(format!("invalid cursor payload: {error}")))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditCursor {
    pub created_at: DateTime<Utc>,
    pub id: Uuid,
}

impl AuditCursor {
    pub fn encode(&self) -> AppResult<String> {
        let json = serde_json::to_vec(self)
            .map_err(|error| AppError::internal(format!("failed to encode cursor: {error}")))?;
        Ok(URL_SAFE_NO_PAD.encode(json))
    }

    pub fn decode(value: &str) -> AppResult<Self> {
        let bytes = URL_SAFE_NO_PAD
            .decode(value)
            .map_err(|error| AppError::Validation(format!("invalid cursor encoding: {error}")))?;
        serde_json::from_slice(&bytes)
            .map_err(|error| AppError::Validation(format!("invalid cursor payload: {error}")))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::{AuditCursor, Cursor};

    #[test]
    fn cursor_round_trip() {
        let cursor = Cursor {
            updated_at: Utc::now(),
            id: Uuid::new_v4(),
        };

        let encoded = cursor.encode().expect("cursor should encode");
        let decoded = Cursor::decode(&encoded).expect("cursor should decode");

        assert_eq!(decoded.id, cursor.id);
        assert_eq!(decoded.updated_at, cursor.updated_at);
    }

    #[test]
    fn audit_cursor_round_trip() {
        let cursor = AuditCursor {
            created_at: Utc::now(),
            id: Uuid::new_v4(),
        };

        let encoded = cursor.encode().expect("audit cursor should encode");
        let decoded = AuditCursor::decode(&encoded).expect("audit cursor should decode");

        assert_eq!(decoded.id, cursor.id);
        assert_eq!(decoded.created_at, cursor.created_at);
    }
}
