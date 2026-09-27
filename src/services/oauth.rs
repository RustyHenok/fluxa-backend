use std::time::Duration;

use serde_json::json;
use uuid::Uuid;

use crate::config::OAuthProviderSettings;
use crate::error::{AppError, AppResult};
use crate::services::audit;
use crate::services::auth::{
    AuthSession, ensure_email_verified, issue_session, resolve_membership,
};
use crate::state::AppState;
use crate::tokens::generate_token;

pub const SUPPORTED_OAUTH_PROVIDERS: [&str; 2] = ["google", "github"];

/// A verified identity returned by an OAuth provider.
#[derive(Debug, Clone)]
struct OAuthIdentity {
    subject: String,
    email: String,
    email_verified: bool,
}

/// Exchanges an authorization code with the provider, resolves the identity,
/// and signs the user in. Unknown identities are linked to an existing user by
/// verified email or auto-provisioned with a fresh tenant, mirroring register.
pub async fn oauth_login(
    state: &AppState,
    provider: &str,
    code: &str,
    redirect_uri: &str,
    tenant_id: Option<Uuid>,
    tenant_name: Option<String>,
) -> AppResult<AuthSession> {
    if !SUPPORTED_OAUTH_PROVIDERS.contains(&provider) {
        return Err(AppError::Validation(format!(
            "unsupported OAuth provider; supported providers: {}",
            SUPPORTED_OAUTH_PROVIDERS.join(", ")
        )));
    }
    let Some(settings) = state.config.oauth_provider(provider) else {
        return Err(AppError::Validation(format!(
            "the {provider} OAuth provider is not configured"
        )));
    };

    let client = oauth_http_client()?;
    let access_token = exchange_code(&client, &settings, code, redirect_uri).await?;
    let identity = fetch_identity(&client, provider, &settings, &access_token).await?;
    let email = normalize_identity_email(&identity.email)?;

    if let Some(user_id) = state
        .db
        .get_oauth_account_user(provider, &identity.subject)
        .await?
    {
        let user = state.db.get_user_by_id(user_id).await?;
        let user = if identity.email_verified && user.email == email {
            state.db.mark_user_email_verified(user.id).await?
        } else {
            user
        };
        ensure_email_verified(state, &user)?;
        let membership = resolve_membership(state, user.id, tenant_id).await?;
        record_login_audit(state, &membership.tenant_id, &user.id, provider).await;
        return issue_session(state, user, membership, Uuid::new_v4()).await;
    }

    // Only trust provider emails the provider itself has verified; otherwise
    // an attacker could link or create accounts for someone else's address.
    if !identity.email_verified {
        return Err(AppError::Forbidden(format!(
            "the {provider} account email address is not verified"
        )));
    }

    if let Some(user) = state.db.get_user_by_email(&email).await? {
        state
            .db
            .link_oauth_account(user.id, provider, &identity.subject)
            .await?;
        let user = state.db.mark_user_email_verified(user.id).await?;
        let membership = resolve_membership(state, user.id, tenant_id).await?;
        audit::record_event(
            state,
            Some(membership.tenant_id),
            Some(user.id),
            "user",
            Some(user.id),
            "auth.oauth_linked",
            json!({ "provider": provider }),
        )
        .await;
        record_login_audit(state, &membership.tenant_id, &user.id, provider).await;
        return issue_session(state, user, membership, Uuid::new_v4()).await;
    }

    let tenant_name = tenant_name
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("{} Workspace", email.split('@').next().unwrap_or("Team")));
    // OAuth-provisioned users get an unguessable random password; password
    // login stays unusable until they run the password reset flow.
    let password_hash = state.auth.hash_password(&generate_token())?;
    let (user, membership) = state
        .db
        .create_user_with_tenant(&email, &password_hash, &tenant_name)
        .await?;
    state
        .db
        .link_oauth_account(user.id, provider, &identity.subject)
        .await?;
    let user = state.db.mark_user_email_verified(user.id).await?;
    audit::record_event(
        state,
        Some(membership.tenant_id),
        Some(user.id),
        "user",
        Some(user.id),
        "user.registered",
        json!({ "method": "oauth", "provider": provider }),
    )
    .await;
    record_login_audit(state, &membership.tenant_id, &user.id, provider).await;
    issue_session(state, user, membership, Uuid::new_v4()).await
}

async fn record_login_audit(state: &AppState, tenant_id: &Uuid, user_id: &Uuid, provider: &str) {
    audit::record_event(
        state,
        Some(*tenant_id),
        Some(*user_id),
        "user",
        Some(*user_id),
        "auth.login_succeeded",
        json!({ "method": "oauth", "provider": provider }),
    )
    .await;
}

fn oauth_http_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("fluxa-backend")
        .build()
        .map_err(|error| AppError::internal(format!("failed to build OAuth client: {error}")))
}

/// Exchanges an authorization code for a provider access token.
async fn exchange_code(
    client: &reqwest::Client,
    settings: &OAuthProviderSettings<'_>,
    code: &str,
    redirect_uri: &str,
) -> AppResult<String> {
    let response = client
        .post(settings.token_url)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", settings.client_id),
            ("client_secret", settings.client_secret),
        ])
        .send()
        .await
        .map_err(|error| AppError::internal(format!("OAuth token request failed: {error}")))?;

    if !response.status().is_success() {
        return Err(AppError::Unauthorized(
            "the OAuth provider rejected the authorization code".into(),
        ));
    }

    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|error| AppError::internal(format!("invalid OAuth token response: {error}")))?;
    payload["access_token"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            AppError::Unauthorized("the OAuth provider did not return an access token".into())
        })
}

async fn fetch_identity(
    client: &reqwest::Client,
    provider: &str,
    settings: &OAuthProviderSettings<'_>,
    access_token: &str,
) -> AppResult<OAuthIdentity> {
    let payload = fetch_userinfo_json(client, settings.userinfo_url, access_token).await?;
    match provider {
        "google" => google_identity(&payload),
        "github" => github_identity(client, settings, access_token, &payload).await,
        _ => Err(AppError::internal("unsupported OAuth provider")),
    }
}

async fn fetch_userinfo_json(
    client: &reqwest::Client,
    url: &str,
    access_token: &str,
) -> AppResult<serde_json::Value> {
    let response = client
        .get(url)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| AppError::internal(format!("OAuth userinfo request failed: {error}")))?;

    if !response.status().is_success() {
        return Err(AppError::Unauthorized(
            "the OAuth provider rejected the access token".into(),
        ));
    }

    response
        .json()
        .await
        .map_err(|error| AppError::internal(format!("invalid OAuth userinfo response: {error}")))
}

fn google_identity(payload: &serde_json::Value) -> AppResult<OAuthIdentity> {
    let subject = payload["sub"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Unauthorized("the OAuth provider did not return a subject".into())
        })?;
    let email = payload["email"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            AppError::Forbidden("the google account does not expose an email address".into())
        })?;
    let email_verified = match &payload["email_verified"] {
        serde_json::Value::Bool(value) => *value,
        serde_json::Value::String(value) => value == "true",
        _ => false,
    };
    Ok(OAuthIdentity {
        subject: subject.to_owned(),
        email: email.to_owned(),
        email_verified,
    })
}

/// GitHub's `/user` payload does not expose email verification, so the
/// primary verified address is resolved via the adjacent `/emails` endpoint.
async fn github_identity(
    client: &reqwest::Client,
    settings: &OAuthProviderSettings<'_>,
    access_token: &str,
    payload: &serde_json::Value,
) -> AppResult<OAuthIdentity> {
    let subject = match &payload["id"] {
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) if !value.is_empty() => value.clone(),
        _ => {
            return Err(AppError::Unauthorized(
                "the OAuth provider did not return a subject".into(),
            ));
        }
    };

    let emails_url = format!("{}/emails", settings.userinfo_url.trim_end_matches('/'));
    let emails = fetch_userinfo_json(client, &emails_url, access_token).await?;
    let entries = emails.as_array().cloned().unwrap_or_default();
    let verified = entries
        .iter()
        .find(|entry| {
            entry["primary"].as_bool().unwrap_or(false)
                && entry["verified"].as_bool().unwrap_or(false)
        })
        .or_else(|| {
            entries
                .iter()
                .find(|entry| entry["verified"].as_bool().unwrap_or(false))
        })
        .and_then(|entry| entry["email"].as_str())
        .filter(|value| !value.is_empty());

    match verified {
        Some(email) => Ok(OAuthIdentity {
            subject,
            email: email.to_owned(),
            email_verified: true,
        }),
        None => Err(AppError::Forbidden(
            "the github account has no verified email address".into(),
        )),
    }
}

fn normalize_identity_email(email: &str) -> AppResult<String> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') {
        return Err(AppError::Unauthorized(
            "the OAuth provider returned an invalid email address".into(),
        ));
    }
    Ok(email)
}
