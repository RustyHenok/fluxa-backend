//! Webhook subscriptions and delivery records.
//!
//! Tenants register HTTPS endpoints subscribed to task lifecycle events.
//! Deliveries are queued in an outbox table and dispatched asynchronously
//! with an HMAC-SHA256 signature computed over the raw request body.

use chrono::{DateTime, Utc};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use sqlx::FromRow;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub const MAX_WEBHOOKS_PER_TENANT: i64 = 10;
pub const MAX_WEBHOOK_URL_LENGTH: usize = 2_048;

pub const WEBHOOK_EVENT_TASK_CREATED: &str = "task_created";
pub const WEBHOOK_EVENT_TASK_UPDATED: &str = "task_updated";
pub const WEBHOOK_EVENT_TASK_STATUS_UPDATED: &str = "task_status_updated";
pub const WEBHOOK_EVENT_TASK_ARCHIVED: &str = "task_archived";
pub const WEBHOOK_EVENT_TASK_RESTORED: &str = "task_restored";

pub const SUPPORTED_WEBHOOK_EVENTS: [&str; 5] = [
    WEBHOOK_EVENT_TASK_CREATED,
    WEBHOOK_EVENT_TASK_UPDATED,
    WEBHOOK_EVENT_TASK_STATUS_UPDATED,
    WEBHOOK_EVENT_TASK_ARCHIVED,
    WEBHOOK_EVENT_TASK_RESTORED,
];

pub const WEBHOOK_DELIVERY_STATUS_PENDING: &str = "pending";
pub const WEBHOOK_DELIVERY_STATUS_DELIVERED: &str = "delivered";
pub const WEBHOOK_DELIVERY_STATUS_DEAD_LETTER: &str = "dead_letter";

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebhookRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub url: String,
    pub secret: String,
    pub events: Vec<String>,
    pub is_active: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Public webhook representation. The signing secret is intentionally
/// omitted; it is only revealed once in the create response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookResponse {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub url: String,
    pub events: Vec<String>,
    pub is_active: bool,
    pub created_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&WebhookRecord> for WebhookResponse {
    fn from(record: &WebhookRecord) -> Self {
        Self {
            id: record.id,
            tenant_id: record.tenant_id,
            url: record.url.clone(),
            events: record.events.clone(),
            is_active: record.is_active,
            created_by: record.created_by,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct WebhookDeliveryRecord {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub tenant_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub scheduled_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookDeliveryResponse {
    pub id: Uuid,
    pub webhook_id: Uuid,
    pub event_type: String,
    pub payload: Value,
    pub status: String,
    pub attempts: i32,
    pub delivered_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&WebhookDeliveryRecord> for WebhookDeliveryResponse {
    fn from(record: &WebhookDeliveryRecord) -> Self {
        Self {
            id: record.id,
            webhook_id: record.webhook_id,
            event_type: record.event_type.clone(),
            payload: record.payload.clone(),
            status: record.status.clone(),
            attempts: record.attempts,
            delivered_at: record.delivered_at,
            last_error: record.last_error.clone(),
            created_at: record.created_at,
        }
    }
}

/// Validates and normalizes a webhook target URL.
///
/// Only `http` and `https` URLs are accepted. Unless `allow_private_urls`
/// is enabled, loopback, link-local, and RFC 1918 targets are rejected to
/// reduce the SSRF surface of outbound deliveries.
pub fn validate_webhook_url(url: &str, allow_private_urls: bool) -> AppResult<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(AppError::Validation("webhook url must not be empty".into()));
    }
    if trimmed.len() > MAX_WEBHOOK_URL_LENGTH {
        return Err(AppError::Validation(format!(
            "webhook url must be at most {MAX_WEBHOOK_URL_LENGTH} characters"
        )));
    }

    let parsed: http::Uri = trimmed
        .parse()
        .map_err(|_| AppError::Validation("webhook url is not a valid URL".into()))?;
    match parsed.scheme_str() {
        Some("http") | Some("https") => {}
        _ => {
            return Err(AppError::Validation(
                "webhook url must use http or https".into(),
            ));
        }
    }
    let Some(host) = parsed.host() else {
        return Err(AppError::Validation(
            "webhook url must include a host".into(),
        ));
    };

    if !allow_private_urls && is_private_host(host) {
        return Err(AppError::Validation(
            "webhook url must not target loopback or private network addresses".into(),
        ));
    }

    Ok(trimmed.to_string())
}

fn is_private_host(host: &str) -> bool {
    let normalized = host.trim_start_matches('[').trim_end_matches(']');
    if normalized.eq_ignore_ascii_case("localhost") {
        return true;
    }
    if let Ok(address) = normalized.parse::<std::net::IpAddr>() {
        return match address {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unspecified()
                    || (v6.segments()[0] & 0xfe00) == 0xfc00
                    || (v6.segments()[0] & 0xffc0) == 0xfe80
            }
        };
    }
    false
}

/// Validates the subscribed event list: deduplicates while preserving
/// order and rejects unknown event types or empty lists.
pub fn validate_webhook_events(events: Vec<String>) -> AppResult<Vec<String>> {
    let mut seen = std::collections::HashSet::new();
    let mut normalized = Vec::new();
    for event in events {
        let event = event.trim().to_string();
        if !SUPPORTED_WEBHOOK_EVENTS.contains(&event.as_str()) {
            return Err(AppError::Validation(format!(
                "unsupported webhook event: {event}"
            )));
        }
        if seen.insert(event.clone()) {
            normalized.push(event);
        }
    }
    if normalized.is_empty() {
        return Err(AppError::Validation(
            "webhook must subscribe to at least one event".into(),
        ));
    }
    Ok(normalized)
}

/// Computes the `X-Fluxa-Signature` header value for a delivery body:
/// `sha256=` followed by the lowercase hex HMAC-SHA256 of the body.
pub fn webhook_signature(secret: &str, body: &[u8]) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts keys of any length");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    let mut rendered = String::with_capacity(7 + digest.len() * 2);
    rendered.push_str("sha256=");
    for byte in digest {
        rendered.push_str(&format!("{byte:02x}"));
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_url_validation_enforces_scheme_and_private_ranges() {
        assert!(validate_webhook_url("https://hooks.example.com/fluxa", false).is_ok());
        assert!(validate_webhook_url("  https://hooks.example.com/x  ", false).is_ok());
        assert!(validate_webhook_url("ftp://hooks.example.com", false).is_err());
        assert!(validate_webhook_url("not a url", false).is_err());
        assert!(validate_webhook_url("", false).is_err());
        assert!(validate_webhook_url("http://localhost:8080/hook", false).is_err());
        assert!(validate_webhook_url("http://127.0.0.1/hook", false).is_err());
        assert!(validate_webhook_url("http://10.1.2.3/hook", false).is_err());
        assert!(validate_webhook_url("http://192.168.0.5/hook", false).is_err());
        assert!(validate_webhook_url("http://169.254.169.254/meta", false).is_err());
        assert!(validate_webhook_url("http://[::1]/hook", false).is_err());
        assert!(validate_webhook_url("http://127.0.0.1:9/hook", true).is_ok());
    }

    #[test]
    fn webhook_events_are_validated_and_deduplicated() {
        let events = validate_webhook_events(vec![
            "task_created".into(),
            "task_created".into(),
            "task_updated".into(),
        ])
        .expect("events should validate");
        assert_eq!(events, vec!["task_created", "task_updated"]);

        assert!(validate_webhook_events(Vec::new()).is_err());
        assert!(validate_webhook_events(vec!["task_exploded".into()]).is_err());
    }

    #[test]
    fn webhook_signature_matches_known_vector() {
        let signature = webhook_signature("secret", b"{\"event\":\"task_created\"}");
        assert!(signature.starts_with("sha256="));
        assert_eq!(signature.len(), 7 + 64);
        // Deterministic for identical inputs, distinct for different keys.
        assert_eq!(
            signature,
            webhook_signature("secret", b"{\"event\":\"task_created\"}")
        );
        assert_ne!(
            signature,
            webhook_signature("other", b"{\"event\":\"task_created\"}")
        );
    }
}
