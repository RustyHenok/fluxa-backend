use fluxa_backend::grpc::proto::job_admin_client::JobAdminClient;
use fluxa_backend::grpc::proto::task_read_client::TaskReadClient;
use fluxa_backend::grpc::proto::{
    EnqueueExportRequest, GetTaskSnapshotRequest, ListTaskSummariesRequest,
    RunDueReminderSweepRequest,
};
use reqwest::Client;
use serde_json::{Value, json};

mod support;

use support::{
    TestServer, add_membership, authed_grpc_request, count_notifications, create_project,
    create_task, fetch_notification_token, insert_stale_running_job, poll_job_status,
    register_user, stack_test_guard, wait_for_job_status, wait_for_rest_job_completion,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn rest_api_enforces_tenant_isolation() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();

    let owner_a = register_user(&client, &server.http_base, "tenant-a").await;
    let owner_b = register_user(&client, &server.http_base, "tenant-b").await;
    let project = create_project(
        &client,
        &server.http_base,
        &owner_b.access_token,
        "Tenant B project",
    )
    .await;
    let project_id = project["id"].as_str().expect("project id should exist");

    let task = create_task(
        &client,
        &server.http_base,
        &owner_b.access_token,
        Some(project_id),
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

    let project_response = client
        .get(format!("{}/v1/projects/{project_id}", server.http_base))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant project fetch should return a response");
    assert_eq!(project_response.status(), reqwest::StatusCode::NOT_FOUND);

    let project_summary_response = client
        .get(format!(
            "{}/v1/projects/{project_id}/summary",
            server.http_base
        ))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant project summary should return a response");
    assert_eq!(
        project_summary_response.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    let project_tasks_response = client
        .get(format!(
            "{}/v1/projects/{project_id}/tasks?limit=10&status=open",
            server.http_base
        ))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant project task list should return a response");
    assert_eq!(
        project_tasks_response.status(),
        reqwest::StatusCode::NOT_FOUND
    );

    let owner_a_summary = client
        .get(format!("{}/v1/dashboard/summary", server.http_base))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("dashboard summary should return a response");
    assert_eq!(owner_a_summary.status(), reqwest::StatusCode::OK);
    let owner_a_summary: Value = owner_a_summary
        .json()
        .await
        .expect("dashboard summary should be json");
    assert_eq!(owner_a_summary["open_task_count"], 0);
    assert_eq!(owner_a_summary["in_progress_task_count"], 0);
    assert_eq!(owner_a_summary["done_task_count"], 0);
    assert_eq!(owner_a_summary["overdue_task_count"], 0);
    assert_eq!(owner_a_summary["recent_activity_count"], 0);

    let cross_tenant_audit = client
        .get(format!(
            "{}/v1/tasks/{task_id}/audit?limit=10",
            server.http_base
        ))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant audit fetch should return a response");
    assert_eq!(cross_tenant_audit.status(), reqwest::StatusCode::NOT_FOUND);

    let export_job = client
        .post(format!("{}/v1/exports/tasks", server.http_base))
        .bearer_auth(&owner_b.access_token)
        .header(
            "Idempotency-Key",
            format!("export-{}", uuid::Uuid::new_v4()),
        )
        .json(&json!({
            "status": "open",
        }))
        .send()
        .await
        .expect("export creation should return a response");
    assert_eq!(export_job.status(), reqwest::StatusCode::ACCEPTED);
    let export_job: Value = export_job
        .json()
        .await
        .expect("export creation response should be json");
    let export_job_id = export_job["id"]
        .as_str()
        .expect("export job id should exist")
        .to_string();

    let finished_export = wait_for_rest_job_completion(
        &client,
        &server.http_base,
        &owner_b.access_token,
        &export_job_id,
    )
    .await;
    assert_eq!(finished_export["status"], "completed");

    let cross_tenant_job_result = client
        .get(format!(
            "{}/v1/jobs/{}/result",
            server.http_base, export_job_id
        ))
        .bearer_auth(&owner_a.access_token)
        .send()
        .await
        .expect("cross-tenant job result should return a response");
    assert_eq!(
        cross_tenant_job_result.status(),
        reqwest::StatusCode::NOT_FOUND
    );

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

    let switched_project = client
        .get(format!("{}/v1/projects/{project_id}", server.http_base))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched tenant project fetch should return a response");
    assert_eq!(switched_project.status(), reqwest::StatusCode::OK);
    let switched_project: Value = switched_project
        .json()
        .await
        .expect("switched project response should be json");
    assert_eq!(switched_project["id"], project_id);

    let switched_project_summary = client
        .get(format!(
            "{}/v1/projects/{project_id}/summary",
            server.http_base
        ))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched project summary should return a response");
    assert_eq!(switched_project_summary.status(), reqwest::StatusCode::OK);
    let switched_project_summary: Value = switched_project_summary
        .json()
        .await
        .expect("switched project summary should be json");
    assert_eq!(switched_project_summary["project_id"], project_id);
    assert_eq!(switched_project_summary["project_name"], "Tenant B project");
    assert_eq!(switched_project_summary["open_task_count"], 1);
    assert_eq!(switched_project_summary["in_progress_task_count"], 0);
    assert_eq!(switched_project_summary["done_task_count"], 0);
    assert_eq!(switched_project_summary["overdue_task_count"], 0);
    assert_eq!(switched_project_summary["recent_activity_count"], 1);

    let switched_project_tasks = client
        .get(format!(
            "{}/v1/projects/{project_id}/tasks?limit=10&status=open",
            server.http_base
        ))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched project task list should return a response");
    assert_eq!(switched_project_tasks.status(), reqwest::StatusCode::OK);
    let switched_project_tasks: Value = switched_project_tasks
        .json()
        .await
        .expect("switched project task list should be json");
    assert_eq!(switched_project_tasks["data"][0]["id"], task_id);
    assert_eq!(switched_project_tasks["data"][0]["project_id"], project_id);

    let switched_summary = client
        .get(format!("{}/v1/dashboard/summary", server.http_base))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched dashboard summary should return a response");
    assert_eq!(switched_summary.status(), reqwest::StatusCode::OK);
    let switched_summary: Value = switched_summary
        .json()
        .await
        .expect("switched dashboard summary should be json");
    assert_eq!(switched_summary["open_task_count"], 1);
    assert_eq!(switched_summary["in_progress_task_count"], 0);
    assert_eq!(switched_summary["done_task_count"], 0);
    assert_eq!(switched_summary["overdue_task_count"], 0);
    assert_eq!(switched_summary["recent_activity_count"], 1);

    let switched_audit = client
        .get(format!(
            "{}/v1/tasks/{task_id}/audit?limit=10",
            server.http_base
        ))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched audit fetch should return a response");
    assert_eq!(switched_audit.status(), reqwest::StatusCode::OK);
    let switched_audit: Value = switched_audit
        .json()
        .await
        .expect("switched audit response should be json");
    assert_eq!(switched_audit["data"][0]["event_type"], "task_created");
    assert_eq!(switched_audit["next_cursor"], Value::Null);

    let switched_job_result = client
        .get(format!(
            "{}/v1/jobs/{}/result",
            server.http_base, export_job_id
        ))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("switched job result should return a response");
    assert_eq!(switched_job_result.status(), reqwest::StatusCode::OK);
    let switched_job_result: Value = switched_job_result
        .json()
        .await
        .expect("switched job result should be json");
    assert_eq!(switched_job_result["job_id"], export_job_id);
    assert_eq!(switched_job_result["job_type"], "task_export");
    assert_eq!(switched_job_result["result"]["task_count"], 1);
    assert_eq!(switched_job_result["result"]["format"], "json");
    let artifact = &switched_job_result["result"]["artifact"];
    assert_eq!(artifact["content_type"], "application/json");
    assert!(
        artifact["size_bytes"].as_i64().unwrap_or_default() > 0,
        "artifact should have a non-zero size"
    );
    let download_path = artifact["download_path"]
        .as_str()
        .expect("export result should include a download path");
    assert_eq!(
        download_path,
        format!("/v1/jobs/{export_job_id}/artifact").as_str()
    );

    let artifact_response = client
        .get(format!("{}{download_path}", server.http_base))
        .bearer_auth(switched_access_token)
        .send()
        .await
        .expect("artifact download should return a response");
    assert_eq!(artifact_response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        artifact_response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let artifact_body: Value = artifact_response
        .json()
        .await
        .expect("artifact should be valid json");
    assert_eq!(artifact_body["tasks"][0]["id"], task_id);

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
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "grpc").await;

    let task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
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
    let unauthenticated = task_read
        .get_task_snapshot(GetTaskSnapshotRequest {
            tenant_id: owner.tenant_id.clone(),
            task_id: task_id.clone(),
        })
        .await
        .expect_err("gRPC requests without an auth token should be rejected");
    assert_eq!(unauthenticated.code(), tonic::Code::Unauthenticated);

    let mut bad_token_request = tonic::Request::new(GetTaskSnapshotRequest {
        tenant_id: owner.tenant_id.clone(),
        task_id: task_id.clone(),
    });
    bad_token_request.metadata_mut().insert(
        "authorization",
        ["Bearer", "wrong-token-wrong-token-wrong-token"]
            .join(" ")
            .parse()
            .expect("metadata should parse"),
    );
    let bad_token = task_read
        .get_task_snapshot(bad_token_request)
        .await
        .expect_err("gRPC requests with a wrong token should be rejected");
    assert_eq!(bad_token.code(), tonic::Code::Unauthenticated);

    let snapshot = task_read
        .get_task_snapshot(authed_grpc_request(GetTaskSnapshotRequest {
            tenant_id: owner.tenant_id.clone(),
            task_id: task_id.clone(),
        }))
        .await
        .expect("GetTaskSnapshot should succeed")
        .into_inner();
    assert_eq!(snapshot.id, task_id);
    assert_eq!(snapshot.title, "gRPC task");
    assert_eq!(snapshot.status, "open");

    let list = task_read
        .list_task_summaries(authed_grpc_request(ListTaskSummariesRequest {
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
        }))
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
        .enqueue_export(authed_grpc_request(EnqueueExportRequest {
            tenant_id: owner.tenant_id.clone(),
            requested_by: owner.user_id.clone(),
            status: "open".into(),
            priority: "high".into(),
            assignee_id: String::new(),
            due_before: String::new(),
            due_after: String::new(),
            updated_after: String::new(),
            q: "gRPC".into(),
        }))
        .await
        .expect("EnqueueExport should succeed")
        .into_inner();

    assert_eq!(job.job_type, "task_export");
    assert_eq!(job.status, "queued");

    let finished = poll_job_status(&mut job_admin, &job.job_id).await;
    assert_eq!(finished.status, "completed");
    assert!(finished.result_payload.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn member_management_enforces_role_rules() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();

    let owner = register_user(&client, &server.http_base, "member-mgmt-owner").await;
    let invitee = register_user(&client, &server.http_base, "member-mgmt-invitee").await;

    let invite_response = client
        .post(format!(
            "{}/v1/tenants/{}/invitations",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "email": invitee.email, "role": "member" }))
        .send()
        .await
        .expect("invitation creation should return a response");
    assert_eq!(invite_response.status(), reqwest::StatusCode::CREATED);
    let invite: Value = invite_response
        .json()
        .await
        .expect("invitation response should be json");
    let invitation_token = invite["token"]
        .as_str()
        .expect("invitation should include a token")
        .to_string();
    assert_eq!(invite["invitation"]["email"], invitee.email);
    assert_eq!(invite["invitation"]["role"], "member");

    let duplicate = client
        .post(format!(
            "{}/v1/tenants/{}/invitations",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "email": invitee.email, "role": "member" }))
        .send()
        .await
        .expect("duplicate invitation should return a response");
    assert_eq!(duplicate.status(), reqwest::StatusCode::CONFLICT);

    let listed = client
        .get(format!(
            "{}/v1/tenants/{}/invitations",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("invitation list should return a response");
    assert_eq!(listed.status(), reqwest::StatusCode::OK);
    let listed: Value = listed.json().await.expect("list should be json");
    assert_eq!(listed.as_array().map(Vec::len), Some(1));

    let wrong_user = register_user(&client, &server.http_base, "member-mgmt-wrong").await;
    let mismatched = client
        .post(format!(
            "{}/v1/tenants/{}/invitations/accept",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&wrong_user.access_token)
        .json(&json!({ "token": invitation_token }))
        .send()
        .await
        .expect("mismatched acceptance should return a response");
    assert_eq!(mismatched.status(), reqwest::StatusCode::FORBIDDEN);

    let accepted = client
        .post(format!(
            "{}/v1/tenants/{}/invitations/accept",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&invitee.access_token)
        .json(&json!({ "token": invitation_token }))
        .send()
        .await
        .expect("acceptance should return a response");
    assert_eq!(accepted.status(), reqwest::StatusCode::OK);
    let membership: Value = accepted.json().await.expect("membership should be json");
    assert_eq!(membership["tenant_id"], owner.tenant_id);
    assert_eq!(membership["role"], "member");

    let replay = client
        .post(format!(
            "{}/v1/tenants/{}/invitations/accept",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&invitee.access_token)
        .json(&json!({ "token": invitation_token }))
        .send()
        .await
        .expect("token replay should return a response");
    assert_eq!(replay.status(), reqwest::StatusCode::NOT_FOUND);

    let switched = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&invitee.access_token)
        .json(&json!({ "tenant_id": owner.tenant_id }))
        .send()
        .await
        .expect("switch-tenant should return a response");
    assert_eq!(switched.status(), reqwest::StatusCode::OK);
    let switched: Value = switched.json().await.expect("switch should be json");
    let member_access = switched["access_token"]
        .as_str()
        .expect("switch should return access token")
        .to_string();

    let forbidden_invite = client
        .post(format!(
            "{}/v1/tenants/{}/invitations",
            server.http_base, owner.tenant_id
        ))
        .bearer_auth(&member_access)
        .json(&json!({ "email": "someone@example.com", "role": "member" }))
        .send()
        .await
        .expect("member invite attempt should return a response");
    assert_eq!(forbidden_invite.status(), reqwest::StatusCode::FORBIDDEN);

    let forbidden_removal = client
        .delete(format!(
            "{}/v1/tenants/{}/members/{}",
            server.http_base, owner.tenant_id, owner.user_id
        ))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("member removal attempt should return a response");
    assert_eq!(forbidden_removal.status(), reqwest::StatusCode::FORBIDDEN);

    let last_owner_demotion = client
        .patch(format!(
            "{}/v1/tenants/{}/members/{}",
            server.http_base, owner.tenant_id, owner.user_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "role": "member" }))
        .send()
        .await
        .expect("last-owner demotion should return a response");
    assert_eq!(last_owner_demotion.status(), reqwest::StatusCode::CONFLICT);

    let promoted = client
        .patch(format!(
            "{}/v1/tenants/{}/members/{}",
            server.http_base, owner.tenant_id, invitee.user_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "role": "admin" }))
        .send()
        .await
        .expect("promotion should return a response");
    assert_eq!(promoted.status(), reqwest::StatusCode::OK);
    let promoted: Value = promoted.json().await.expect("promotion should be json");
    assert_eq!(promoted["role"], "admin");

    let removed = client
        .delete(format!(
            "{}/v1/tenants/{}/members/{}",
            server.http_base, owner.tenant_id, invitee.user_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("member removal should return a response");
    assert_eq!(removed.status(), reqwest::StatusCode::NO_CONTENT);

    let removed_member_access = client
        .get(format!("{}/v1/dashboard/summary", server.http_base))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("removed member access should return a response");
    assert_eq!(
        removed_member_access.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn stale_running_jobs_are_reaped() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "reaper").await;

    let export_payload = json!({
        "tenant_id": owner.tenant_id,
        "requested_by": owner.user_id,
        "filters": {},
    });

    let recoverable_job = insert_stale_running_job(&owner.tenant_id, &export_payload, 1, 5).await;
    let recovered = wait_for_job_status(
        &client,
        &server.http_base,
        &owner.access_token,
        &recoverable_job,
        &["completed"],
    )
    .await;
    assert_eq!(recovered["status"], "completed");

    let exhausted_job = insert_stale_running_job(&owner.tenant_id, &export_payload, 5, 5).await;
    let dead_lettered = wait_for_job_status(
        &client,
        &server.http_base,
        &owner.access_token,
        &exhausted_job,
        &["dead_letter"],
    )
    .await;
    assert_eq!(dead_lettered["status"], "dead_letter");
    assert_eq!(
        dead_lettered["last_error"],
        "job lease expired before completion"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn logout_denylists_access_token() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "logout-denylist").await;

    let me_before = client
        .get(format!("{}/v1/me", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("/v1/me before logout should return a response");
    assert_eq!(me_before.status(), reqwest::StatusCode::OK);

    let logout = client
        .post(format!("{}/v1/auth/logout", server.http_base))
        .json(&json!({
            "refresh_token": owner.refresh_token,
            "access_token": owner.access_token,
        }))
        .send()
        .await
        .expect("logout should return a response");
    assert_eq!(logout.status(), reqwest::StatusCode::NO_CONTENT);

    let refresh_after = client
        .post(format!("{}/v1/auth/refresh", server.http_base))
        .json(&json!({ "refresh_token": owner.refresh_token }))
        .send()
        .await
        .expect("refresh after logout should return a response");
    assert_eq!(refresh_after.status(), reqwest::StatusCode::UNAUTHORIZED);

    let me_after = client
        .get(format!("{}/v1/me", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("/v1/me after logout should return a response");
    assert_eq!(me_after.status(), reqwest::StatusCode::UNAUTHORIZED);
    let body: Value = me_after.json().await.expect("error should be json");
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn account_lifecycle_flows() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "account").await;

    let weak_password = client
        .post(format!("{}/v1/auth/register", server.http_base))
        .json(&json!({
            "email": format!("weak-{}@example.com", uuid::Uuid::new_v4()),
            "password": "password123",
            "tenant_name": "Weak Workspace",
        }))
        .send()
        .await
        .expect("weak password register should return a response");
    assert_eq!(weak_password.status(), reqwest::StatusCode::BAD_REQUEST);
    let weak_body: Value = weak_password
        .json()
        .await
        .expect("weak password error should be json");
    assert_eq!(weak_body["error"]["code"], "validation_error");

    let me_before: Value = client
        .get(format!("{}/v1/me", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("/v1/me should return a response")
        .json()
        .await
        .expect("me response should be json");
    assert_eq!(me_before["user"]["email_verified"], false);

    let verify_token = fetch_notification_token(&owner.email, "email_verification").await;
    let verified = client
        .post(format!("{}/v1/auth/verify-email", server.http_base))
        .json(&json!({ "token": verify_token }))
        .send()
        .await
        .expect("verify email should return a response");
    assert_eq!(verified.status(), reqwest::StatusCode::NO_CONTENT);

    let me_after: Value = client
        .get(format!("{}/v1/me", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("/v1/me should return a response")
        .json()
        .await
        .expect("me response should be json");
    assert_eq!(me_after["user"]["email_verified"], true);

    let reused_token = client
        .post(format!("{}/v1/auth/verify-email", server.http_base))
        .json(&json!({ "token": verify_token }))
        .send()
        .await
        .expect("verify email reuse should return a response");
    assert_eq!(reused_token.status(), reqwest::StatusCode::UNAUTHORIZED);

    let reset_request = client
        .post(format!(
            "{}/v1/auth/password-reset/request",
            server.http_base
        ))
        .json(&json!({ "email": owner.email }))
        .send()
        .await
        .expect("password reset request should return a response");
    assert_eq!(reset_request.status(), reqwest::StatusCode::ACCEPTED);

    let unknown_reset = client
        .post(format!(
            "{}/v1/auth/password-reset/request",
            server.http_base
        ))
        .json(&json!({ "email": format!("nobody-{}@example.com", uuid::Uuid::new_v4()) }))
        .send()
        .await
        .expect("unknown email reset request should return a response");
    assert_eq!(unknown_reset.status(), reqwest::StatusCode::ACCEPTED);

    let reset_token = fetch_notification_token(&owner.email, "password_reset").await;
    let reset_confirm = client
        .post(format!(
            "{}/v1/auth/password-reset/confirm",
            server.http_base
        ))
        .json(&json!({
            "token": reset_token,
            "new_password": "fresh-secret-pw-1",
        }))
        .send()
        .await
        .expect("password reset confirm should return a response");
    assert_eq!(reset_confirm.status(), reqwest::StatusCode::NO_CONTENT);

    let stale_refresh = client
        .post(format!("{}/v1/auth/refresh", server.http_base))
        .json(&json!({ "refresh_token": owner.refresh_token }))
        .send()
        .await
        .expect("stale refresh should return a response");
    assert_eq!(stale_refresh.status(), reqwest::StatusCode::UNAUTHORIZED);

    let old_password_login = client
        .post(format!("{}/v1/auth/login", server.http_base))
        .json(&json!({
            "email": owner.email,
            "password": "supersecret123",
        }))
        .send()
        .await
        .expect("old password login should return a response");
    assert_eq!(
        old_password_login.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );

    let login: Value = client
        .post(format!("{}/v1/auth/login", server.http_base))
        .json(&json!({
            "email": owner.email,
            "password": "fresh-secret-pw-1",
        }))
        .send()
        .await
        .expect("new password login should return a response")
        .json()
        .await
        .expect("login response should be json");
    let login_access = login["access_token"]
        .as_str()
        .expect("login should include access token")
        .to_string();
    let login_refresh = login["refresh_token"]
        .as_str()
        .expect("login should include refresh token")
        .to_string();

    let change_password = client
        .post(format!("{}/v1/me/change-password", server.http_base))
        .bearer_auth(&login_access)
        .json(&json!({
            "current_password": "fresh-secret-pw-1",
            "new_password": "changed-secret-pw-2",
        }))
        .send()
        .await
        .expect("change password should return a response");
    assert_eq!(change_password.status(), reqwest::StatusCode::NO_CONTENT);

    let revoked_refresh = client
        .post(format!("{}/v1/auth/refresh", server.http_base))
        .json(&json!({ "refresh_token": login_refresh }))
        .send()
        .await
        .expect("revoked refresh should return a response");
    assert_eq!(revoked_refresh.status(), reqwest::StatusCode::UNAUTHORIZED);

    let relogin: Value = client
        .post(format!("{}/v1/auth/login", server.http_base))
        .json(&json!({
            "email": owner.email,
            "password": "changed-secret-pw-2",
        }))
        .send()
        .await
        .expect("relogin should return a response")
        .json()
        .await
        .expect("relogin response should be json");
    let relogin_access = relogin["access_token"]
        .as_str()
        .expect("relogin should include access token")
        .to_string();

    let new_email = format!("changed-{}@example.com", uuid::Uuid::new_v4());
    let change_email = client
        .post(format!("{}/v1/me/change-email", server.http_base))
        .bearer_auth(&relogin_access)
        .json(&json!({
            "current_password": "changed-secret-pw-2",
            "new_email": new_email,
        }))
        .send()
        .await
        .expect("change email should return a response");
    assert_eq!(change_email.status(), reqwest::StatusCode::OK);
    let changed_user: Value = change_email
        .json()
        .await
        .expect("change email response should be json");
    assert_eq!(changed_user["email"], new_email.as_str());
    assert_eq!(changed_user["email_verified"], false);

    let new_email_token = fetch_notification_token(&new_email, "email_verification").await;
    assert!(!new_email_token.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn audit_log_restricted_to_admins() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "audit-owner").await;
    let outsider = register_user(&client, &server.http_base, "audit-outsider").await;

    create_project(&client, &server.http_base, &owner.access_token, "Audited").await;

    let audit_response = client
        .get(format!("{}/v1/audit?limit=50", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("audit list should return a response");
    assert_eq!(audit_response.status(), reqwest::StatusCode::OK);
    let audit: Value = audit_response
        .json()
        .await
        .expect("audit response should be json");
    let events = audit["data"]
        .as_array()
        .expect("audit data should be array");
    assert!(
        events
            .iter()
            .any(|event| event["event_type"] == "user.registered"),
        "audit log should contain the registration event"
    );
    assert!(
        events
            .iter()
            .any(|event| event["event_type"] == "project.created"),
        "audit log should contain the project creation event"
    );
    assert!(
        events
            .iter()
            .all(|event| event["actor_user_id"] != outsider.user_id.as_str()),
        "audit log must not leak another tenant's events"
    );

    add_membership(&outsider.user_id, &owner.tenant_id, "member").await;
    let switched: Value = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&outsider.access_token)
        .json(&json!({ "tenant_id": owner.tenant_id }))
        .send()
        .await
        .expect("switch tenant should return a response")
        .json()
        .await
        .expect("switch tenant response should be json");
    let member_access = switched["access_token"]
        .as_str()
        .expect("switch should include access token");

    let member_audit = client
        .get(format!("{}/v1/audit", server.http_base))
        .bearer_auth(member_access)
        .send()
        .await
        .expect("member audit list should return a response");
    assert_eq!(member_audit.status(), reqwest::StatusCode::FORBIDDEN);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn csv_export_produces_downloadable_artifact() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "csv-export").await;

    create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
        "CSV export task",
        "open",
        "high",
    )
    .await;

    let export_job = client
        .post(format!("{}/v1/exports/tasks", server.http_base))
        .bearer_auth(&owner.access_token)
        .header(
            "Idempotency-Key",
            format!("export-{}", uuid::Uuid::new_v4()),
        )
        .json(&json!({ "format": "csv" }))
        .send()
        .await
        .expect("csv export creation should return a response");
    assert_eq!(export_job.status(), reqwest::StatusCode::ACCEPTED);
    let export_job: Value = export_job
        .json()
        .await
        .expect("csv export response should be json");
    let job_id = export_job["id"]
        .as_str()
        .expect("export job id should exist")
        .to_string();

    wait_for_rest_job_completion(&client, &server.http_base, &owner.access_token, &job_id).await;

    let result: Value = client
        .get(format!("{}/v1/jobs/{job_id}/result", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("csv export result should return a response")
        .json()
        .await
        .expect("csv export result should be json");
    assert_eq!(result["result"]["format"], "csv");
    assert_eq!(result["result"]["task_count"], 1);
    assert_eq!(result["result"]["artifact"]["content_type"], "text/csv");

    let artifact = client
        .get(format!("{}/v1/jobs/{job_id}/artifact", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("csv artifact download should return a response");
    assert_eq!(artifact.status(), reqwest::StatusCode::OK);
    assert_eq!(
        artifact
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/csv")
    );
    let body = artifact
        .text()
        .await
        .expect("csv artifact should be readable");
    let mut lines = body.lines();
    let header = lines.next().expect("csv should have a header row");
    assert!(header.starts_with("id,"), "unexpected csv header: {header}");
    assert!(
        lines.any(|line| line.contains("CSV export task")),
        "csv should contain the exported task"
    );

    let anonymous_artifact = client
        .get(format!("{}/v1/jobs/{job_id}/artifact", server.http_base))
        .send()
        .await
        .expect("anonymous artifact download should return a response");
    assert_eq!(
        anonymous_artifact.status(),
        reqwest::StatusCode::UNAUTHORIZED
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn due_reminder_sweep_notifies_assignees() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "reminder-notify").await;

    let due_at = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    let task = client
        .post(format!("{}/v1/tasks", server.http_base))
        .bearer_auth(&owner.access_token)
        .header("Idempotency-Key", format!("task-{}", uuid::Uuid::new_v4()))
        .json(&json!({
            "title": "Due soon task",
            "status": "open",
            "priority": "high",
            "assignee_id": owner.user_id,
            "due_at": due_at,
        }))
        .send()
        .await
        .expect("task creation should return a response");
    assert_eq!(task.status(), reqwest::StatusCode::CREATED);

    let mut job_admin = JobAdminClient::connect(server.grpc_base.clone())
        .await
        .expect("grpc job client should connect");

    let sweep = job_admin
        .run_due_reminder_sweep(authed_grpc_request(RunDueReminderSweepRequest {
            tenant_id: owner.tenant_id.clone(),
        }))
        .await
        .expect("RunDueReminderSweep should succeed")
        .into_inner();
    let finished = poll_job_status(&mut job_admin, &sweep.job_id).await;
    assert_eq!(
        sweep_notification_count(&finished),
        1.0,
        "sweep should report one enqueued notification"
    );
    assert_eq!(
        count_notifications(&owner.email, "task_due_soon").await,
        1,
        "sweep should enqueue one due-soon notification"
    );

    let second_sweep = job_admin
        .run_due_reminder_sweep(authed_grpc_request(RunDueReminderSweepRequest {
            tenant_id: owner.tenant_id.clone(),
        }))
        .await
        .expect("second RunDueReminderSweep should succeed")
        .into_inner();
    let second_finished = poll_job_status(&mut job_admin, &second_sweep.job_id).await;
    assert_eq!(
        sweep_notification_count(&second_finished),
        0.0,
        "repeat sweep should be deduplicated"
    );
    assert_eq!(
        count_notifications(&owner.email, "task_due_soon").await,
        1,
        "dedupe should keep a single due-soon notification"
    );
}

fn sweep_notification_count(reply: &fluxa_backend::grpc::proto::JobReply) -> f64 {
    let payload = reply
        .result_payload
        .as_ref()
        .expect("sweep should include a result payload");
    match payload
        .fields
        .get("notification_count")
        .and_then(|value| value.kind.as_ref())
    {
        Some(prost_types::value::Kind::NumberValue(number)) => *number,
        other => panic!("notification_count should be a number, got {other:?}"),
    }
}
