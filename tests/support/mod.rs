use std::fs::File;
use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fluxa_backend::grpc::proto::job_admin_client::JobAdminClient;
use fluxa_backend::grpc::proto::{GetJobStatusRequest, JobReply};
use once_cell::sync::Lazy;
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::sleep;
use tonic::transport::Channel;
use uuid::Uuid;

const DEFAULT_TEST_DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:5432/fluxa";
const DEFAULT_TEST_REDIS_URL: &str = "redis://127.0.0.1:16379/";
const TEST_JWT_SECRET: &str = "integration-test-secret-integration-test";
pub const TEST_GRPC_AUTH_TOKEN: &str = "integration-test-grpc-token-integration-test";
static STACK_TEST_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

pub async fn stack_test_guard() -> MutexGuard<'static, ()> {
    STACK_TEST_MUTEX.lock().await
}

pub struct TestServer {
    child: Child,
    pub http_base: String,
    pub grpc_base: String,
    stdout_path: std::path::PathBuf,
    stderr_path: std::path::PathBuf,
    artifact_dir: std::path::PathBuf,
}

impl TestServer {
    pub async fn start() -> Self {
        let http_port = free_port();
        let grpc_port = free_port();
        let http_base = format!("http://127.0.0.1:{http_port}");
        let grpc_base = format!("http://127.0.0.1:{grpc_port}");
        let stdout_path = log_path("stack-contract-stdout");
        let stderr_path = log_path("stack-contract-stderr");
        let artifact_dir =
            std::env::temp_dir().join(format!("stack-contract-artifacts-{}", Uuid::new_v4()));
        let stdout = File::create(&stdout_path).expect("failed to create test stdout log");
        let stderr = File::create(&stderr_path).expect("failed to create test stderr log");

        let child = Command::new(test_binary())
            .arg("--mode")
            .arg("all")
            .env(
                "DATABASE_URL",
                std::env::var("TEST_DATABASE_URL")
                    .unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string()),
            )
            .env(
                "REDIS_URL",
                std::env::var("TEST_REDIS_URL")
                    .unwrap_or_else(|_| DEFAULT_TEST_REDIS_URL.to_string()),
            )
            .env("JWT_SECRET", TEST_JWT_SECRET)
            .env("GRPC_AUTH_TOKEN", TEST_GRPC_AUTH_TOKEN)
            .env("JOB_LEASE_SECONDS", "3")
            .env("WORKER_DISPATCH_INTERVAL_MS", "500")
            .env("WORKER_SCHEDULER_INTERVAL_MS", "1000")
            .env("SAMPLER_INTERVAL_MS", "500")
            .env("MAILER_PROVIDER", "log")
            .env("NOTIFY_DISPATCH_INTERVAL_MS", "250")
            .env("ARTIFACT_STORAGE_DIR", &artifact_dir)
            .env("HTTP_ADDR", format!("127.0.0.1:{http_port}"))
            .env("GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
            .env("STARTUP_MAX_RETRIES", "10")
            .env("STARTUP_RETRY_DELAY_MS", "500")
            .env("DATABASE_MAX_CONNECTIONS", "4")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("failed to spawn fluxa-backend test server");

        let mut server = Self {
            child,
            http_base,
            grpc_base,
            stdout_path,
            stderr_path,
            artifact_dir,
        };
        server.wait_for_ready().await;
        server
    }

    async fn wait_for_ready(&mut self) {
        let client = Client::new();
        let deadline = Instant::now() + Duration::from_secs(30);

        loop {
            if Instant::now() > deadline {
                panic!(
                    "test server did not become ready in time\n{}",
                    self.debug_output()
                );
            }

            if let Some(status) = self.child.try_wait().expect("failed to poll child") {
                panic!(
                    "test server exited before becoming ready: {status}\n{}",
                    self.debug_output()
                );
            }

            match client
                .get(format!("{}/readyz", self.http_base))
                .send()
                .await
            {
                Ok(response) if response.status().is_success() => {
                    let payload: Value = response.json().await.expect("readyz should return json");
                    if payload["status"] == "ready" {
                        return;
                    }
                }
                _ => {}
            }

            sleep(Duration::from_millis(500)).await;
        }
    }

    fn debug_output(&self) -> String {
        format!(
            "stdout:\n{}\n\nstderr:\n{}",
            read_log(&self.stdout_path),
            read_log(&self.stderr_path)
        )
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.stdout_path);
        let _ = std::fs::remove_file(&self.stderr_path);
        let _ = std::fs::remove_dir_all(&self.artifact_dir);
    }
}

pub struct RegisteredUser {
    pub access_token: String,
    pub refresh_token: String,
    pub tenant_id: String,
    pub user_id: String,
    pub email: String,
}

pub async fn add_membership(user_id: &str, tenant_id: &str, role: &str) {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    sqlx::query(
        r#"
        INSERT INTO tenant_memberships (tenant_id, user_id, role, created_at)
        VALUES ($1, $2, $3, now())
        "#,
    )
    .bind(Uuid::parse_str(tenant_id).expect("tenant id should be uuid"))
    .bind(Uuid::parse_str(user_id).expect("user id should be uuid"))
    .bind(role)
    .execute(&pool)
    .await
    .expect("membership insert should succeed");
}

pub async fn register_user(client: &Client, base: &str, label: &str) -> RegisteredUser {
    let email = unique_email(label);
    let response = client
        .post(format!("{base}/v1/auth/register"))
        .json(&json!({
            "email": email,
            "password": "supersecret123",
            "tenant_name": format!("{label} Workspace"),
        }))
        .send()
        .await
        .expect("register request should succeed");

    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    let body: Value = response.json().await.expect("register should return json");

    RegisteredUser {
        access_token: body["access_token"]
            .as_str()
            .expect("register should include access token")
            .to_string(),
        refresh_token: body["refresh_token"]
            .as_str()
            .expect("register should include refresh token")
            .to_string(),
        tenant_id: body["active_tenant"]["tenant_id"]
            .as_str()
            .expect("register should include tenant id")
            .to_string(),
        user_id: body["user"]["id"]
            .as_str()
            .expect("register should include user id")
            .to_string(),
        email,
    }
}

pub async fn create_project(client: &Client, base: &str, access_token: &str, name: &str) -> Value {
    let response = client
        .post(format!("{base}/v1/projects"))
        .bearer_auth(access_token)
        .header("Idempotency-Key", format!("project-{}", Uuid::new_v4()))
        .json(&json!({
            "name": name,
            "description": "integration coverage",
        }))
        .send()
        .await
        .expect("create project request should succeed");

    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response
        .json()
        .await
        .expect("project response should be json")
}

pub async fn create_task(
    client: &Client,
    base: &str,
    access_token: &str,
    project_id: Option<&str>,
    title: &str,
    status: &str,
    priority: &str,
) -> Value {
    let response = client
        .post(format!("{base}/v1/tasks"))
        .bearer_auth(access_token)
        .header("Idempotency-Key", format!("task-{}", Uuid::new_v4()))
        .json(&json!({
            "project_id": project_id,
            "title": title,
            "description": "integration coverage",
            "status": status,
            "priority": priority,
        }))
        .send()
        .await
        .expect("create task request should succeed");

    assert_eq!(response.status(), reqwest::StatusCode::CREATED);
    response.json().await.expect("task response should be json")
}

pub async fn poll_job_status(client: &mut JobAdminClient<Channel>, job_id: &str) -> JobReply {
    let deadline = Instant::now() + Duration::from_secs(20);

    loop {
        let response = client
            .get_job_status(authed_grpc_request(GetJobStatusRequest {
                job_id: job_id.to_string(),
            }))
            .await
            .expect("get_job_status should succeed")
            .into_inner();

        if response.status == "completed" {
            return response;
        }

        if Instant::now() > deadline {
            panic!("job {job_id} did not complete in time");
        }

        sleep(Duration::from_millis(500)).await;
    }
}

pub async fn wait_for_rest_job_completion(
    client: &Client,
    base: &str,
    access_token: &str,
    job_id: &str,
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);

    loop {
        let response = client
            .get(format!("{base}/v1/jobs/{job_id}"))
            .bearer_auth(access_token)
            .send()
            .await
            .expect("job status request should succeed");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Value = response
            .json()
            .await
            .expect("job status response should be json");

        match body["status"].as_str() {
            Some("completed") => return body,
            Some("dead_letter") => panic!("job {job_id} dead-lettered unexpectedly"),
            _ => {}
        }

        if Instant::now() > deadline {
            panic!("job {job_id} did not complete in time");
        }

        sleep(Duration::from_millis(500)).await;
    }
}

pub fn authed_grpc_request<T>(message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    let header = format!("Bearer {TEST_GRPC_AUTH_TOKEN}");
    request.metadata_mut().insert(
        "authorization",
        header.parse().expect("grpc auth header should parse"),
    );
    request
}

pub async fn fetch_notification_token(recipient: &str, kind: &str) -> String {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let token: Option<String> = sqlx::query_scalar(
            r#"
            SELECT payload->>'token'
            FROM notifications
            WHERE recipient = $1 AND kind = $2
            ORDER BY created_at DESC
            LIMIT 1
            "#,
        )
        .bind(recipient)
        .bind(kind)
        .fetch_optional(&pool)
        .await
        .expect("notification lookup should succeed")
        .flatten();

        if let Some(token) = token {
            return token;
        }

        if Instant::now() > deadline {
            panic!("no {kind} notification for {recipient} appeared in time");
        }

        sleep(Duration::from_millis(250)).await;
    }
}

pub async fn count_notifications(recipient: &str, kind: &str) -> i64 {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM notifications
        WHERE recipient = $1 AND kind = $2
        "#,
    )
    .bind(recipient)
    .bind(kind)
    .fetch_one(&pool)
    .await
    .expect("notification count should succeed")
}

pub async fn insert_stale_running_job(
    tenant_id: &str,
    payload: &Value,
    attempts: i32,
    max_attempts: i32,
) -> String {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    let job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO background_jobs
            (id, tenant_id, job_type, payload, status, attempts, max_attempts,
             scheduled_at, started_at)
        VALUES
            ($1, $2, 'task_export', $3, 'running', $4, $5,
             now() - interval '10 minutes', now() - interval '10 minutes')
        "#,
    )
    .bind(job_id)
    .bind(Uuid::parse_str(tenant_id).expect("tenant id should be uuid"))
    .bind(payload)
    .bind(attempts)
    .bind(max_attempts)
    .execute(&pool)
    .await
    .expect("stale job insert should succeed");

    job_id.to_string()
}

pub struct RetentionFixtures {
    pub token_id: String,
    pub job_id: String,
    pub notification_id: String,
    pub audit_id: String,
}

pub async fn insert_retention_fixtures(user_id: &str, tenant_id: &str) -> RetentionFixtures {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    let user = Uuid::parse_str(user_id).expect("user id should be uuid");
    let tenant = Uuid::parse_str(tenant_id).expect("tenant id should be uuid");

    let token_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO refresh_tokens (id, user_id, tenant_id, expires_at, created_at)
        VALUES ($1, $2, $3, now() - interval '60 days', now() - interval '61 days')
        "#,
    )
    .bind(token_id)
    .bind(user)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("expired refresh token insert should succeed");

    let job_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO background_jobs
            (id, tenant_id, job_type, payload, status, attempts, max_attempts,
             scheduled_at, started_at, finished_at)
        VALUES
            ($1, $2, 'task_export', '{}'::jsonb, 'completed', 1, 3,
             now() - interval '60 days', now() - interval '60 days',
             now() - interval '60 days')
        "#,
    )
    .bind(job_id)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("old terminal job insert should succeed");

    let notification_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO notifications
            (id, tenant_id, user_id, kind, recipient, payload, status,
             attempts, max_attempts, scheduled_at, sent_at, created_at)
        VALUES
            ($1, $2, $3, 'task_due_soon', 'retention@test.local', '{}'::jsonb,
             'sent', 1, 5, now() - interval '60 days', now() - interval '60 days',
             now() - interval '60 days')
        "#,
    )
    .bind(notification_id)
    .bind(tenant)
    .bind(user)
    .execute(&pool)
    .await
    .expect("old sent notification insert should succeed");

    let audit_id = Uuid::new_v4();
    sqlx::query(
        r#"
        INSERT INTO audit_log
            (id, tenant_id, actor_user_id, subject_type, subject_id, event_type,
             payload, created_at)
        VALUES
            ($1, $2, $3, 'auth', $3, 'auth.login', '{}'::jsonb,
             now() - interval '400 days')
        "#,
    )
    .bind(audit_id)
    .bind(tenant)
    .bind(user)
    .execute(&pool)
    .await
    .expect("old audit event insert should succeed");

    sqlx::query(
        "DELETE FROM background_jobs WHERE job_type = 'retention_sweep' AND status <> 'running'",
    )
    .execute(&pool)
    .await
    .expect("clearing prior retention jobs should succeed");

    RetentionFixtures {
        token_id: token_id.to_string(),
        job_id: job_id.to_string(),
        notification_id: notification_id.to_string(),
        audit_id: audit_id.to_string(),
    }
}

pub async fn retention_leftover_count(fixtures: &RetentionFixtures) -> i64 {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&test_database_url())
        .await
        .expect("test database should be reachable");

    let count: i64 = sqlx::query_scalar(
        r#"
        SELECT
            (SELECT count(*) FROM refresh_tokens WHERE id = $1::uuid)
          + (SELECT count(*) FROM background_jobs WHERE id = $2::uuid)
          + (SELECT count(*) FROM notifications WHERE id = $3::uuid)
          + (SELECT count(*) FROM audit_log WHERE id = $4::uuid)
        "#,
    )
    .bind(Uuid::parse_str(&fixtures.token_id).expect("token id should be uuid"))
    .bind(Uuid::parse_str(&fixtures.job_id).expect("job id should be uuid"))
    .bind(Uuid::parse_str(&fixtures.notification_id).expect("notification id should be uuid"))
    .bind(Uuid::parse_str(&fixtures.audit_id).expect("audit id should be uuid"))
    .fetch_one(&pool)
    .await
    .expect("retention leftover count should succeed");

    count
}

pub async fn wait_for_job_status(
    client: &Client,
    base: &str,
    access_token: &str,
    job_id: &str,
    statuses: &[&str],
) -> Value {
    let deadline = Instant::now() + Duration::from_secs(20);

    loop {
        let response = client
            .get(format!("{base}/v1/jobs/{job_id}"))
            .bearer_auth(access_token)
            .send()
            .await
            .expect("job status request should succeed");

        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body: Value = response
            .json()
            .await
            .expect("job status response should be json");

        if let Some(status) = body["status"].as_str()
            && statuses.contains(&status)
        {
            return body;
        }

        if Instant::now() > deadline {
            panic!("job {job_id} did not reach one of {statuses:?} in time; last: {body}");
        }

        sleep(Duration::from_millis(250)).await;
    }
}

fn test_binary() -> &'static str {
    env!("CARGO_BIN_EXE_fluxa-backend")
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("should bind ephemeral port")
        .local_addr()
        .expect("should read local addr")
        .port()
}

fn unique_email(label: &str) -> String {
    format!("{label}-{}@example.com", Uuid::new_v4())
}

fn test_database_url() -> String {
    std::env::var("TEST_DATABASE_URL").unwrap_or_else(|_| DEFAULT_TEST_DATABASE_URL.to_string())
}

fn log_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}.log", Uuid::new_v4()))
}

fn read_log(path: &std::path::Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| format!("<failed to read {}: {error}>", path.display()))
}
