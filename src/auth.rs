use std::sync::Arc;

use argon2::Argon2;
use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng,
};
use chrono::{Duration as ChronoDuration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::SharedConfig;
use crate::domain::{MembershipRecord, UserRecord};
use crate::error::{AppError, AppResult};

#[derive(Clone)]
pub struct AuthService {
    config: SharedConfig,
    encoding_key: Arc<EncodingKey>,
    decoding_key: Arc<DecodingKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenClaims {
    pub sub: String,
    pub tenant_id: String,
    pub role: String,
    pub token_type: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in_seconds: u64,
}

impl AuthService {
    pub fn new(config: SharedConfig) -> AppResult<Self> {
        let encoding_key = EncodingKey::from_secret(config.jwt_secret.as_bytes());
        let decoding_key = DecodingKey::from_secret(config.jwt_secret.as_bytes());

        Ok(Self {
            config,
            encoding_key: Arc::new(encoding_key),
            decoding_key: Arc::new(decoding_key),
        })
    }

    pub fn hash_password(&self, password: &str) -> AppResult<String> {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|hash| hash.to_string())
            .map_err(|error| AppError::internal(format!("failed to hash password: {error}")))
    }

    pub fn verify_password(&self, password: &str, password_hash: &str) -> AppResult<()> {
        let parsed = PasswordHash::new(password_hash)
            .map_err(|error| AppError::Unauthorized(format!("invalid password hash: {error}")))?;
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .map_err(|_| AppError::Unauthorized("invalid credentials".into()))
    }

    pub fn issue_token_pair(
        &self,
        user: &UserRecord,
        membership: &MembershipRecord,
        refresh_token_id: Uuid,
    ) -> AppResult<TokenPair> {
        let now = Utc::now();
        let access_exp = now
            + ChronoDuration::from_std(self.config.access_token_ttl()).map_err(|error| {
                AppError::internal(format!("invalid access token ttl: {error}"))
            })?;
        let refresh_exp = now
            + ChronoDuration::from_std(self.config.refresh_token_ttl()).map_err(|error| {
                AppError::internal(format!("invalid refresh token ttl: {error}"))
            })?;

        let access_claims = TokenClaims {
            sub: user.id.to_string(),
            tenant_id: membership.tenant_id.to_string(),
            role: membership.role.clone(),
            token_type: "access".into(),
            jti: Uuid::new_v4().to_string(),
            iat: now.timestamp(),
            exp: access_exp.timestamp(),
        };

        let refresh_claims = TokenClaims {
            sub: user.id.to_string(),
            tenant_id: membership.tenant_id.to_string(),
            role: membership.role.clone(),
            token_type: "refresh".into(),
            jti: refresh_token_id.to_string(),
            iat: now.timestamp(),
            exp: refresh_exp.timestamp(),
        };

        Ok(TokenPair {
            access_token: encode(&Header::default(), &access_claims, &self.encoding_key)?,
            refresh_token: encode(&Header::default(), &refresh_claims, &self.encoding_key)?,
            expires_in_seconds: self.config.access_token_ttl().as_secs(),
        })
    }

    pub fn decode_access_token(&self, token: &str) -> AppResult<TokenClaims> {
        self.decode_token(token, "access")
    }

    pub fn decode_refresh_token(&self, token: &str) -> AppResult<TokenClaims> {
        self.decode_token(token, "refresh")
    }

    fn decode_token(&self, token: &str, expected_type: &str) -> AppResult<TokenClaims> {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        let token = decode::<TokenClaims>(token, &self.decoding_key, &validation)?;
        if token.claims.token_type != expected_type {
            return Err(AppError::Unauthorized(format!(
                "expected {expected_type} token"
            )));
        }

        Ok(token.claims)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use uuid::Uuid;

    use crate::config::{Cli, ServiceMode};
    use crate::domain::{MembershipRecord, ROLE_OWNER, UserRecord};

    use super::AuthService;

    fn config() -> Arc<Cli> {
        Arc::new(
            Cli {
                app_name: "test".into(),
                mode: ServiceMode::Api,
                http_addr: "127.0.0.1:8080".parse().unwrap(),
                grpc_addr: "127.0.0.1:50051".parse().unwrap(),
                database_url: "postgres://localhost/test".into(),
                redis_url: "redis://localhost/".into(),
                jwt_secret: "super-secret-key-super-secret-key".into(),
                grpc_auth_token: "grpc-shared-secret-grpc-shared-secret".into(),
                access_token_minutes: 15,
                refresh_token_days: 30,
                cache_ttl_seconds: 60,
                idempotency_ttl_seconds: 60,
                auth_rate_limit_capacity: 10,
                auth_rate_limit_refill_per_sec: 1,
                app_rate_limit_capacity: 10,
                app_rate_limit_refill_per_sec: 1,
                job_queue_name: "jobs".into(),
                worker_dispatch_interval_ms: 1000,
                worker_scheduler_interval_ms: 1000,
                job_queue_block_timeout_seconds: 1,
                max_job_attempts: 3,
                job_lease_seconds: 300,
                invitation_ttl_hours: 72,
                database_max_connections: 1,
                cors_allow_origin: "*".into(),
                startup_max_retries: 3,
                startup_retry_delay_ms: 100,
                mailer_provider: "noop".into(),
                smtp_url: None,
                mail_from: None,
                notify_dispatch_interval_ms: 1000,
                require_email_verification: false,
                email_verification_ttl_hours: 24,
                password_reset_ttl_minutes: 60,
                reminder_due_soon_hours: 24,
                reminder_dedupe_ttl_hours: 24,
                artifact_storage_dir: "data/exports".into(),
                max_attachment_size_bytes: 5_242_880,
                webhook_dispatch_interval_ms: 5_000,
                webhook_allow_private_urls: false,
                otel_exporter_otlp_endpoint: None,
                otel_service_name: "fluxa-backend".into(),
                oauth_google_client_id: None,
                oauth_google_client_secret: None,
                oauth_google_token_url: "https://oauth2.googleapis.com/token".into(),
                oauth_google_userinfo_url: "https://openidconnect.googleapis.com/v1/userinfo"
                    .into(),
                oauth_github_client_id: None,
                oauth_github_client_secret: None,
                oauth_github_token_url: "https://github.com/login/oauth/access_token".into(),
                oauth_github_userinfo_url: "https://api.github.com/user".into(),
                retention_sweep_interval_hours: 24,
                refresh_token_retention_days: 30,
                job_retention_days: 30,
                notification_retention_days: 30,
                audit_retention_days: 365,
                sampler_interval_ms: 10_000,
                metrics_auth_token: None,
            }
            .validate()
            .unwrap(),
        )
    }

    #[test]
    fn access_and_refresh_tokens_decode() {
        let service = AuthService::new(config()).unwrap();
        let user = UserRecord {
            id: Uuid::new_v4(),
            email: "test@example.com".into(),
            password_hash: "hash".into(),
            display_name: None,
            email_verified_at: None,
            created_at: Utc::now(),
        };
        let membership = MembershipRecord {
            tenant_id: Uuid::new_v4(),
            tenant_name: "Tenant".into(),
            user_id: user.id,
            role: ROLE_OWNER.into(),
            created_at: Utc::now(),
        };

        let pair = service
            .issue_token_pair(&user, &membership, Uuid::new_v4())
            .unwrap();

        let access = service.decode_access_token(&pair.access_token).unwrap();
        let refresh = service.decode_refresh_token(&pair.refresh_token).unwrap();

        assert_eq!(access.sub, user.id.to_string());
        assert_eq!(refresh.tenant_id, membership.tenant_id.to_string());
    }

    #[test]
    fn otlp_endpoint_ignores_blank_values() {
        let base = config();
        assert_eq!(base.otlp_endpoint(), None);

        let blank = Cli {
            otel_exporter_otlp_endpoint: Some("   ".into()),
            ..(*base).clone()
        };
        assert_eq!(blank.otlp_endpoint(), None);

        let configured = Cli {
            otel_exporter_otlp_endpoint: Some(" http://collector:4317 ".into()),
            ..(*base).clone()
        };
        assert_eq!(configured.otlp_endpoint(), Some("http://collector:4317"));
    }

    #[test]
    fn oauth_provider_requires_client_id_and_secret() {
        let base = config();
        assert!(base.oauth_provider("google").is_none());
        assert!(base.oauth_provider("github").is_none());
        assert!(base.oauth_provider("gitlab").is_none());

        let partial = Cli {
            oauth_google_client_id: Some("client".into()),
            oauth_google_client_secret: Some("   ".into()),
            ..(*base).clone()
        };
        assert!(partial.oauth_provider("google").is_none());

        let configured = Cli {
            oauth_google_client_id: Some(" client ".into()),
            oauth_google_client_secret: Some("secret".into()),
            ..(*base).clone()
        };
        let settings = configured
            .oauth_provider("google")
            .expect("google should be configured");
        assert_eq!(settings.client_id, "client");
        assert_eq!(settings.client_secret, "secret");
        assert_eq!(settings.token_url, "https://oauth2.googleapis.com/token");
        assert!(configured.oauth_provider("github").is_none());
    }
}
