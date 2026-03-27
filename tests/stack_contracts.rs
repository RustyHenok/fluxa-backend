use fluxa_backend::grpc::proto::job_admin_client::JobAdminClient;
use fluxa_backend::grpc::proto::task_read_client::TaskReadClient;
use fluxa_backend::grpc::proto::{
    EnqueueExportRequest, GetTaskSnapshotRequest, ListTaskSummariesRequest,
};
use reqwest::Client;
use serde_json::{Value, json};

mod support;

use support::{TestServer, add_membership, create_task, poll_job_status, register_user};

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

    let member_list_for_other_tenant = client
        .get(format!(
            "{}/v1/tenants/{}/members",
            server.http_base, owner_b.tenant_id
        ))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant member list should return a response");
    assert_eq!(
        member_list_for_other_tenant.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    add_membership(&owner_a.user_id, &owner_b.tenant_id, "member").await;

    let switched = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&owner_a.access_token)
        .json(&json!({
            "tenant_id": owner_b.tenant_id,
        }))
        .send()
        .await
        .expect("switch-tenant should return a response");

    assert_eq!(switched.status(), reqwest::StatusCode::OK);
    let switched_body: Value = switched
        .json()
        .await
        .expect("switch-tenant response should be json");
    assert_eq!(
        switched_body["active_tenant"]["tenant_id"],
        owner_b.tenant_id
    );
    assert_eq!(switched_body["active_tenant"]["role"], "member");
    let switched_access_token = switched_body["access_token"]
        .as_str()
        .expect("switch-tenant should return an access token");

    let switched_fetch = client
        .get(format!("{}/v1/tasks/{task_id}", server.http_base))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched tenant task fetch should return a response");

    assert_eq!(switched_fetch.status(), reqwest::StatusCode::OK);

    let member_list = client
        .get(format!(
            "{}/v1/tenants/{}/members",
            server.http_base, owner_b.tenant_id
        ))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("same-tenant member list should return a response");

    assert_eq!(member_list.status(), reqwest::StatusCode::OK);
    let members: Value = member_list
        .json()
        .await
        .expect("member list response should be json");
    let members = members.as_array().expect("member list should be an array");
    assert_eq!(members.len(), 2);
    assert!(
        members.iter().any(|member| {
            member["user_id"] == owner_b.user_id
                && member["email"] == owner_b.email
                && member["role"] == "owner"
        }),
        "tenant owner should appear in member list"
    );
    assert!(
        members.iter().any(|member| {
            member["user_id"] == owner_a.user_id
                && member["email"] == owner_a.email
                && member["role"] == "member"
        }),
        "switched member should appear in member list"
    );

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
