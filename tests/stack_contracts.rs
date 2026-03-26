use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fluxa_backend::grpc::proto::job_admin_client::JobAdminClient;
use fluxa_backend::grpc::proto::task_read_client::TaskReadClient;
use fluxa_backend::grpc::proto::{
    EnqueueExportRequest, GetJobStatusRequest, GetTaskSnapshotRequest, ListTaskSummariesRequest,
};
use reqwest::Client;
use serde_json::{Value, json};
use tokio::time::sleep;
use tonic::transport::Channel;
use uuid::Uuid;

const DEFAULT_TEST_DATABASE_URL: &str = "postgres://postgres:postgres@127.0.0.1:5432/fluxa";
const DEFAULT_TEST_REDIS_URL: &str = "redis://127.0.0.1:16379/";
const TEST_JWT_SECRET: &str = "integration-test-secret-integration-test";

struct TestServer {
    child: Child,
    http_base: String,
    grpc_base: String,
}

impl TestServer {
    async fn start() -> Self {
        let http_port = free_port();
        let grpc_port = free_port();
        let http_base = format!("http://127.0.0.1:{http_port}");
        let grpc_base = format!("http://127.0.0.1:{grpc_port}");

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
            .env("HTTP_ADDR", format!("127.0.0.1:{http_port}"))
            .env("GRPC_ADDR", format!("127.0.0.1:{grpc_port}"))
            .env("STARTUP_MAX_RETRIES", "10")
            .env("STARTUP_RETRY_DELAY_MS", "500")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn fluxa-backend test server");

        let mut server = Self {
            child,
            http_base,
            grpc_base,
        };
        server.wait_for_ready().await;
        server
    }

    async fn wait_for_ready(&mut self) {
        let client = Client::new();
        let deadline = Instant::now() + Duration::from_secs(30);

        loop {
            if Instant::now() > deadline {
                panic!("test server did not become ready in time");
            }

            if let Some(status) = self.child.try_wait().expect("failed to poll child") {
                panic!("test server exited before becoming ready: {status}");
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
}

impl Drop for TestServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct RegisteredUser {
    access_token: String,
    refresh_token: String,
    tenant_id: String,
    user_id: String,
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

async fn register_user(client: &Client, base: &str, label: &str) -> RegisteredUser {
    let response = client
        .post(format!("{base}/v1/auth/register"))
        .json(&json!({
            "email": unique_email(label),
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
    }
}

async fn create_task(
    client: &Client,
    base: &str,
    access_token: &str,
    title: &str,
    status: &str,
    priority: &str,
) -> Value {
    let response = client
        .post(format!("{base}/v1/tasks"))
        .bearer_auth(access_token)
        .header("Idempotency-Key", format!("task-{}", Uuid::new_v4()))
        .json(&json!({
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

async fn poll_job_status(
    client: &mut JobAdminClient<Channel>,
    job_id: &str,
) -> fluxa_backend::grpc::proto::JobReply {
    let deadline = Instant::now() + Duration::from_secs(20);

    loop {
        let response = client
            .get_job_status(GetJobStatusRequest {
                job_id: job_id.to_string(),
            })
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn rest_api_enforces_tenant_isolation() {
    let server = TestServer::start().await;
    let client = Client::new();

    let owner_a = register_user(&client, &server.http_base, "tenant-a").await;
    let owner_b = register_user(&client, &server.http_base, "tenant-b").await;

    let task = create_task(
        &client,
        &server.http_base,
        &owner_b.access_token,
        "Tenant B only task",
        "open",
        "high",
    )
    .await;
    let task_id = task["id"].as_str().expect("task id should exist");

    let response = client
        .get(format!("{}/v1/tasks/{task_id}", server.http_base))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant task fetch should return a response");

    assert_eq!(response.status(), reqwest::StatusCode::NOT_FOUND);
    let body: Value = response
        .json()
        .await
        .expect("error response should be json");
    assert_eq!(body["error"]["code"], "not_found");

    let me_response = client
        .get(format!("{}/v1/me", server.http_base))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("/v1/me should succeed");
    assert_eq!(me_response.status(), reqwest::StatusCode::OK);

    let refresh_response = client
        .post(format!("{}/v1/auth/refresh", server.http_base))
        .json(&json!({
            "refresh_token": owner_a.refresh_token,
        }))
        .send()
        .await
        .expect("refresh should succeed");
    assert_eq!(refresh_response.status(), reqwest::StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn grpc_contracts_expose_tasks_and_jobs() {
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "grpc").await;

    let task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        "gRPC task",
        "open",
        "high",
    )
    .await;
    let task_id = task["id"]
        .as_str()
        .expect("task id should exist")
        .to_string();

    let mut task_read = TaskReadClient::connect(server.grpc_base.clone())
        .await
        .expect("grpc task client should connect");
    let snapshot = task_read
        .get_task_snapshot(GetTaskSnapshotRequest {
            tenant_id: owner.tenant_id.clone(),
            task_id: task_id.clone(),
        })
        .await
        .expect("GetTaskSnapshot should succeed")
        .into_inner();
    assert_eq!(snapshot.id, task_id);
    assert_eq!(snapshot.title, "gRPC task");
    assert_eq!(snapshot.status, "open");

    let list = task_read
        .list_task_summaries(ListTaskSummariesRequest {
            tenant_id: owner.tenant_id.clone(),
            limit: 10,
            cursor: String::new(),
            status: "open".into(),
            priority: "high".into(),
            assignee_id: String::new(),
            due_before: String::new(),
            due_after: String::new(),
            updated_after: String::new(),
            q: "gRPC".into(),
        })
        .await
        .expect("ListTaskSummaries should succeed")
        .into_inner();
    assert!(
        list.tasks.iter().any(|task| task.id == task_id),
        "task list should include the task created through REST"
    );

    let mut job_admin = JobAdminClient::connect(server.grpc_base.clone())
        .await
        .expect("grpc job client should connect");
    let job = job_admin
        .enqueue_export(EnqueueExportRequest {
            tenant_id: owner.tenant_id.clone(),
            requested_by: owner.user_id.clone(),
            status: "open".into(),
            priority: "high".into(),
            assignee_id: String::new(),
            due_before: String::new(),
            due_after: String::new(),
            updated_after: String::new(),
            q: "gRPC".into(),
        })
        .await
        .expect("EnqueueExport should succeed")
        .into_inner();

    assert_eq!(job.job_type, "task_export");
    assert_eq!(job.status, "queued");

    let finished = poll_job_status(&mut job_admin, &job.job_id).await;
    assert_eq!(finished.status, "completed");
    assert!(finished.result_payload.is_some());
}
