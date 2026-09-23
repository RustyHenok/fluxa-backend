//! Outbound notification delivery. Notifications are enqueued into the
//! `notifications` outbox table by services and drained by the worker's
//! notifier loop, which renders and sends them through the configured
//! [`Mailer`] provider (`noop` by default, `log` for local development,
//! `smtp` for real delivery).

use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde_json::Value;
use tracing::info;

use crate::config::Cli;
use crate::domain::NotificationRecord;
use crate::error::{AppError, AppResult};

pub const KIND_TENANT_INVITATION: &str = "tenant_invitation";
pub const KIND_EMAIL_VERIFICATION: &str = "email_verification";
pub const KIND_PASSWORD_RESET: &str = "password_reset";
pub const KIND_TASK_DUE_SOON: &str = "task_due_soon";
pub const KIND_TASK_OVERDUE: &str = "task_overdue";

#[derive(Debug, Clone)]
pub struct MailMessage {
    pub to: String,
    pub subject: String,
    pub body: String,
}

pub trait Mailer {
    fn send(&self, message: &MailMessage) -> impl Future<Output = AppResult<()>> + Send;
}

#[derive(Debug, Clone, Default)]
pub struct NoopMailer;

impl Mailer for NoopMailer {
    async fn send(&self, _message: &MailMessage) -> AppResult<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct LogMailer;

impl Mailer for LogMailer {
    async fn send(&self, message: &MailMessage) -> AppResult<()> {
        info!(
            to = %message.to,
            subject = %message.subject,
            body = %message.body,
            "mail delivered via log mailer"
        );
        Ok(())
    }
}

#[derive(Clone)]
pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpMailer {
    pub fn from_config(config: &Cli) -> AppResult<Self> {
        let url = config
            .smtp_url
            .as_deref()
            .ok_or_else(|| AppError::Validation("SMTP_URL is not configured".into()))?;
        let from = config
            .mail_from
            .as_deref()
            .ok_or_else(|| AppError::Validation("MAIL_FROM is not configured".into()))?
            .parse::<Mailbox>()
            .map_err(|error| AppError::Validation(format!("invalid MAIL_FROM: {error}")))?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
            .map_err(|error| AppError::Validation(format!("invalid SMTP_URL: {error}")))?
            .build();

        Ok(Self { transport, from })
    }
}

impl Mailer for SmtpMailer {
    async fn send(&self, message: &MailMessage) -> AppResult<()> {
        let to = message
            .to
            .parse::<Mailbox>()
            .map_err(|error| AppError::Validation(format!("invalid recipient: {error}")))?;
        let email = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(message.subject.clone())
            .body(message.body.clone())
            .map_err(|error| AppError::internal(format!("failed to build email: {error}")))?;

        self.transport
            .send(email)
            .await
            .map_err(|error| AppError::internal(format!("smtp delivery failed: {error}")))?;
        Ok(())
    }
}

#[derive(Clone)]
pub enum AnyMailer {
    Noop(NoopMailer),
    Log(LogMailer),
    Smtp(Box<SmtpMailer>),
}

impl AnyMailer {
    pub fn from_config(config: &Cli) -> AppResult<Self> {
        match config.mailer_provider.as_str() {
            "noop" => Ok(Self::Noop(NoopMailer)),
            "log" => Ok(Self::Log(LogMailer)),
            "smtp" => Ok(Self::Smtp(Box::new(SmtpMailer::from_config(config)?))),
            other => Err(AppError::Validation(format!(
                "unsupported mailer provider: {other}"
            ))),
        }
    }
}

impl Mailer for AnyMailer {
    async fn send(&self, message: &MailMessage) -> AppResult<()> {
        match self {
            Self::Noop(mailer) => mailer.send(message).await,
            Self::Log(mailer) => mailer.send(message).await,
            Self::Smtp(mailer) => mailer.send(message).await,
        }
    }
}

/// Renders an outbox row into a sendable message. Unknown kinds fail so they
/// surface via the retry/dead-letter path instead of silently dropping.
pub fn render_notification(notification: &NotificationRecord) -> AppResult<MailMessage> {
    let payload = &notification.payload;
    let message = match notification.kind.as_str() {
        KIND_TENANT_INVITATION => MailMessage {
            to: notification.recipient.clone(),
            subject: "You have been invited to a workspace".into(),
            body: format!(
                "You have been invited to join a workspace with the {} role.\n\
                 Use this invitation token to accept: {}\n\
                 The invitation expires at {}.",
                payload_str(payload, "role"),
                payload_str(payload, "token"),
                payload_str(payload, "expires_at"),
            ),
        },
        KIND_EMAIL_VERIFICATION => MailMessage {
            to: notification.recipient.clone(),
            subject: "Verify your email address".into(),
            body: format!(
                "Use this code to verify your email address: {}\n\
                 The code expires at {}.",
                payload_str(payload, "token"),
                payload_str(payload, "expires_at"),
            ),
        },
        KIND_PASSWORD_RESET => MailMessage {
            to: notification.recipient.clone(),
            subject: "Reset your password".into(),
            body: format!(
                "Use this code to reset your password: {}\n\
                 The code expires at {}. If you did not request this, ignore this message.",
                payload_str(payload, "token"),
                payload_str(payload, "expires_at"),
            ),
        },
        KIND_TASK_DUE_SOON => MailMessage {
            to: notification.recipient.clone(),
            subject: format!("Task due soon: {}", payload_str(payload, "title")),
            body: format!(
                "The task \"{}\" is due at {}.",
                payload_str(payload, "title"),
                payload_str(payload, "due_at"),
            ),
        },
        KIND_TASK_OVERDUE => MailMessage {
            to: notification.recipient.clone(),
            subject: format!("Task overdue: {}", payload_str(payload, "title")),
            body: format!(
                "The task \"{}\" was due at {} and is now overdue.",
                payload_str(payload, "title"),
                payload_str(payload, "due_at"),
            ),
        },
        other => {
            return Err(AppError::internal(format!(
                "unsupported notification kind: {other}"
            )));
        }
    };

    Ok(message)
}

fn payload_str<'a>(payload: &'a Value, key: &str) -> &'a str {
    payload.get(key).and_then(Value::as_str).unwrap_or("")
}
