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
    create_task, fetch_notification_token, insert_retention_fixtures, insert_stale_running_job,
    poll_job_status, register_user, retention_leftover_count, stack_test_guard,
    wait_for_job_status, wait_for_rest_job_completion,
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
        .header(
            "Idempotency-Key",
            format!("invite-{}", uuid::Uuid::new_v4()),
        )
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
        .header(
            "Idempotency-Key",
            format!("invite-{}", uuid::Uuid::new_v4()),
        )
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn retention_sweep_purges_old_rows_and_metrics_expose_route_series() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "retention").await;

    let fixtures = insert_retention_fixtures(&owner.user_id, &owner.tenant_id).await;
    assert_eq!(
        retention_leftover_count(&fixtures).await,
        4,
        "all retention fixtures should exist before the sweep"
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if retention_leftover_count(&fixtures).await == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "retention sweep should purge fixture rows within the deadline"
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // Give the sampler at least one tick before scraping metrics.
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;

    let metrics = client
        .get(format!("{}/metrics", server.http_base))
        .send()
        .await
        .expect("metrics request should succeed");
    assert_eq!(metrics.status(), reqwest::StatusCode::OK);
    let body = metrics.text().await.expect("metrics body should be text");

    assert!(
        body.contains("http_requests_total{")
            && body.contains("route=\"/v1/auth/register\"")
            && body.contains("http_request_duration_seconds"),
        "metrics should expose per-route counters and latency histograms"
    );
    assert!(
        body.contains("db_pool_connections{"),
        "metrics should expose DB pool gauges"
    );
    assert!(
        body.contains("retention_rows_purged_total{"),
        "metrics should count purged retention rows"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn soft_delete_restore_idempotency_and_search() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "soft-delete").await;

    // Idempotent project creation: same key replays the original response.
    let project_key = format!("project-{}", uuid::Uuid::new_v4());
    let mut project_ids = Vec::new();
    for _ in 0..2 {
        let response = client
            .post(format!("{}/v1/projects", server.http_base))
            .bearer_auth(&owner.access_token)
            .header("Idempotency-Key", &project_key)
            .json(&json!({ "name": "Replay project", "description": "idempotency" }))
            .send()
            .await
            .expect("project create should return a response");
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        let body: Value = response.json().await.expect("project should be json");
        project_ids.push(body["id"].as_str().expect("project id").to_string());
    }
    assert_eq!(
        project_ids[0], project_ids[1],
        "same idempotency key should replay the same project"
    );

    let missing_key = client
        .post(format!("{}/v1/projects", server.http_base))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "name": "No key" }))
        .send()
        .await
        .expect("missing key request should return a response");
    assert_eq!(missing_key.status(), reqwest::StatusCode::BAD_REQUEST);

    // Task soft delete and restore.
    let project = create_project(&client, &server.http_base, &owner.access_token, "Live").await;
    let project_id = project["id"].as_str().expect("project id").to_string();
    let task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        Some(&project_id),
        "Quarterly Budget Review",
        "open",
        "high",
    )
    .await;
    let task_id = task["id"].as_str().expect("task id").to_string();

    let archived = client
        .delete(format!("{}/v1/tasks/{}", server.http_base, task_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task delete should return a response");
    assert_eq!(archived.status(), reqwest::StatusCode::NO_CONTENT);

    let fetched = client
        .get(format!("{}/v1/tasks/{}", server.http_base, task_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("archived task fetch should return a response");
    assert_eq!(fetched.status(), reqwest::StatusCode::OK);
    let fetched: Value = fetched.json().await.expect("task should be json");
    assert_eq!(fetched["status"], "archived", "delete should archive");

    let restored = client
        .post(format!("{}/v1/tasks/{}/restore", server.http_base, task_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task restore should return a response");
    assert_eq!(restored.status(), reqwest::StatusCode::OK);
    let restored: Value = restored.json().await.expect("restore should be json");
    assert_eq!(restored["status"], "open", "restore should reopen the task");

    let restore_again = client
        .post(format!("{}/v1/tasks/{}/restore", server.http_base, task_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("second restore should return a response");
    assert_eq!(restore_again.status(), reqwest::StatusCode::NOT_FOUND);

    // Full-text search plus short-term substring fallback.
    create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        Some(&project_id),
        "Standup notes",
        "open",
        "low",
    )
    .await;

    let fts = client
        .get(format!("{}/v1/tasks?q=budget", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("search should return a response");
    assert_eq!(fts.status(), reqwest::StatusCode::OK);
    let fts: Value = fts.json().await.expect("search should be json");
    let titles: Vec<&str> = fts["data"]
        .as_array()
        .expect("search data should be an array")
        .iter()
        .filter_map(|task| task["title"].as_str())
        .collect();
    assert!(
        titles.contains(&"Quarterly Budget Review") && !titles.contains(&"Standup notes"),
        "full-text search should match whole words only, got {titles:?}"
    );

    let short = client
        .get(format!("{}/v1/tasks?q=Bu", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("short search should return a response");
    let short: Value = short.json().await.expect("short search should be json");
    assert!(
        short["data"]
            .as_array()
            .expect("short search data should be an array")
            .iter()
            .any(|task| task["title"] == "Quarterly Budget Review"),
        "short terms should substring-match"
    );

    // Project soft delete hides the project and its tasks until restore.
    let hidden_project =
        create_project(&client, &server.http_base, &owner.access_token, "Hidden").await;
    let hidden_id = hidden_project["id"].as_str().expect("project id");
    let hidden_task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        Some(hidden_id),
        "Invisible work item",
        "open",
        "medium",
    )
    .await;
    let hidden_task_id = hidden_task["id"].as_str().expect("task id");

    let archive_project = client
        .delete(format!("{}/v1/projects/{}", server.http_base, hidden_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("project delete should return a response");
    assert_eq!(archive_project.status(), reqwest::StatusCode::NO_CONTENT);

    let project_gone = client
        .get(format!("{}/v1/projects/{}", server.http_base, hidden_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("archived project fetch should return a response");
    assert_eq!(project_gone.status(), reqwest::StatusCode::NOT_FOUND);

    let listing = client
        .get(format!("{}/v1/tasks?limit=100", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task listing should return a response");
    let listing: Value = listing.json().await.expect("listing should be json");
    assert!(
        !listing["data"]
            .as_array()
            .expect("listing data should be an array")
            .iter()
            .any(|task| task["id"] == hidden_task_id),
        "tasks of archived projects should be hidden from listings"
    );

    let restore_project = client
        .post(format!(
            "{}/v1/projects/{}/restore",
            server.http_base, hidden_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("project restore should return a response");
    assert_eq!(restore_project.status(), reqwest::StatusCode::OK);

    let listing = client
        .get(format!("{}/v1/tasks?limit=100", server.http_base))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task listing after restore should return a response");
    let listing: Value = listing.json().await.expect("listing should be json");
    assert!(
        listing["data"]
            .as_array()
            .expect("listing data should be an array")
            .iter()
            .any(|task| task["id"] == hidden_task_id),
        "restoring the project should make its tasks visible again"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn labels_management_and_task_filtering() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "labels-owner").await;
    let member = register_user(&client, &server.http_base, "labels-member").await;

    add_membership(&member.user_id, &owner.tenant_id, "member").await;
    let switched: Value = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&member.access_token)
        .json(&json!({ "tenant_id": owner.tenant_id }))
        .send()
        .await
        .expect("switch tenant should return a response")
        .json()
        .await
        .expect("switch tenant response should be json");
    let member_access = switched["access_token"]
        .as_str()
        .expect("switch should include access token")
        .to_string();

    // Idempotent label creation replays the original response.
    let label_key = format!("label-{}", uuid::Uuid::new_v4());
    let mut bug_ids = Vec::new();
    for _ in 0..2 {
        let response = client
            .post(format!("{}/v1/labels", server.http_base))
            .bearer_auth(&owner.access_token)
            .header("Idempotency-Key", &label_key)
            .json(&json!({ "name": "Bug", "color": "#FF0000" }))
            .send()
            .await
            .expect("label create should return a response");
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        let body: Value = response.json().await.expect("label should be json");
        assert_eq!(
            body["color"], "#ff0000",
            "color should normalize to lowercase"
        );
        bug_ids.push(body["id"].as_str().expect("label id").to_string());
    }
    assert_eq!(bug_ids[0], bug_ids[1], "same key should replay the label");
    let bug_id = bug_ids[0].clone();

    let missing_key = client
        .post(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "name": "No key" }))
        .send()
        .await
        .expect("missing key request should return a response");
    assert_eq!(missing_key.status(), reqwest::StatusCode::BAD_REQUEST);

    let duplicate = client
        .post(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&owner.access_token)
        .header("Idempotency-Key", format!("label-{}", uuid::Uuid::new_v4()))
        .json(&json!({ "name": "bug" }))
        .send()
        .await
        .expect("duplicate label create should return a response");
    assert_eq!(
        duplicate.status(),
        reqwest::StatusCode::CONFLICT,
        "label names should be unique per tenant, case-insensitively"
    );

    let bad_color = client
        .post(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&owner.access_token)
        .header("Idempotency-Key", format!("label-{}", uuid::Uuid::new_v4()))
        .json(&json!({ "name": "Ugly", "color": "red" }))
        .send()
        .await
        .expect("invalid color create should return a response");
    assert_eq!(bad_color.status(), reqwest::StatusCode::BAD_REQUEST);

    let member_create = client
        .post(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&member_access)
        .header("Idempotency-Key", format!("label-{}", uuid::Uuid::new_v4()))
        .json(&json!({ "name": "Member label" }))
        .send()
        .await
        .expect("member label create should return a response");
    assert_eq!(member_create.status(), reqwest::StatusCode::FORBIDDEN);

    let feature: Value = client
        .post(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&owner.access_token)
        .header("Idempotency-Key", format!("label-{}", uuid::Uuid::new_v4()))
        .json(&json!({ "name": "Feature" }))
        .send()
        .await
        .expect("feature label create should return a response")
        .json()
        .await
        .expect("feature label should be json");
    let feature_id = feature["id"].as_str().expect("label id").to_string();

    let listed: Value = client
        .get(format!("{}/v1/labels", server.http_base))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("label list should return a response")
        .json()
        .await
        .expect("label list should be json");
    let names: Vec<&str> = listed
        .as_array()
        .expect("label list should be an array")
        .iter()
        .filter_map(|label| label["name"].as_str())
        .collect();
    assert_eq!(names, vec!["Bug", "Feature"], "labels should sort by name");

    // Members can attach labels; the set is replaced wholesale.
    let task_one = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
        "Fix login crash",
        "open",
        "high",
    )
    .await;
    let task_one_id = task_one["id"].as_str().expect("task id").to_string();
    let task_two = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
        "Ship dark mode",
        "open",
        "medium",
    )
    .await;
    let task_two_id = task_two["id"].as_str().expect("task id").to_string();

    let assigned: Value = client
        .put(format!(
            "{}/v1/tasks/{}/labels",
            server.http_base, task_one_id
        ))
        .bearer_auth(&member_access)
        .json(&json!({ "label_ids": [bug_id] }))
        .send()
        .await
        .expect("label assignment should return a response")
        .json()
        .await
        .expect("label assignment should be json");
    assert_eq!(assigned.as_array().map(Vec::len), Some(1));
    assert_eq!(assigned[0]["name"], "Bug");

    client
        .put(format!(
            "{}/v1/tasks/{}/labels",
            server.http_base, task_two_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "label_ids": [bug_id, feature_id] }))
        .send()
        .await
        .expect("second label assignment should return a response");

    let unknown_label = client
        .put(format!(
            "{}/v1/tasks/{}/labels",
            server.http_base, task_one_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "label_ids": [uuid::Uuid::new_v4()] }))
        .send()
        .await
        .expect("unknown label assignment should return a response");
    assert_eq!(unknown_label.status(), reqwest::StatusCode::NOT_FOUND);

    // label_id filters listings down to tagged tasks.
    let filtered: Value = client
        .get(format!(
            "{}/v1/tasks?label_id={}",
            server.http_base, feature_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("label filter should return a response")
        .json()
        .await
        .expect("label filter should be json");
    let filtered_ids: Vec<&str> = filtered["data"]
        .as_array()
        .expect("filtered data should be an array")
        .iter()
        .filter_map(|task| task["id"].as_str())
        .collect();
    assert_eq!(
        filtered_ids,
        vec![task_two_id.as_str()],
        "feature filter should only match the second task"
    );

    let bug_filtered: Value = client
        .get(format!("{}/v1/tasks?label_id={}", server.http_base, bug_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("bug filter should return a response")
        .json()
        .await
        .expect("bug filter should be json");
    assert_eq!(
        bug_filtered["data"].as_array().map(Vec::len),
        Some(2),
        "bug filter should match both tasks"
    );

    // Rename and recolor, then delete; deletion detaches the label everywhere.
    let renamed: Value = client
        .patch(format!("{}/v1/labels/{}", server.http_base, bug_id))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "name": "Defect", "color": "#00FF00" }))
        .send()
        .await
        .expect("label rename should return a response")
        .json()
        .await
        .expect("label rename should be json");
    assert_eq!(renamed["name"], "Defect");
    assert_eq!(renamed["color"], "#00ff00");

    let member_delete = client
        .delete(format!("{}/v1/labels/{}", server.http_base, feature_id))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("member label delete should return a response");
    assert_eq!(member_delete.status(), reqwest::StatusCode::FORBIDDEN);

    let deleted = client
        .delete(format!("{}/v1/labels/{}", server.http_base, feature_id))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("label delete should return a response");
    assert_eq!(deleted.status(), reqwest::StatusCode::NO_CONTENT);

    let task_two_labels: Value = client
        .get(format!(
            "{}/v1/tasks/{}/labels",
            server.http_base, task_two_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task labels should return a response")
        .json()
        .await
        .expect("task labels should be json");
    let remaining: Vec<&str> = task_two_labels
        .as_array()
        .expect("task labels should be an array")
        .iter()
        .filter_map(|label| label["name"].as_str())
        .collect();
    assert_eq!(
        remaining,
        vec!["Defect"],
        "deleting a label should detach it from tasks"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn task_comments_crud_permissions_and_notifications() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "comments-owner").await;
    let member = register_user(&client, &server.http_base, "comments-member").await;

    add_membership(&member.user_id, &owner.tenant_id, "member").await;
    let switched: Value = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&member.access_token)
        .json(&json!({ "tenant_id": owner.tenant_id }))
        .send()
        .await
        .expect("switch tenant should return a response")
        .json()
        .await
        .expect("switch tenant response should be json");
    let member_access = switched["access_token"]
        .as_str()
        .expect("switch should include access token")
        .to_string();

    // Task owned by the owner, assigned to the owner so comments trigger a
    // notification when someone else comments.
    let task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
        "Comment target",
        "open",
        "medium",
    )
    .await;
    let task_id = task["id"].as_str().expect("task id").to_string();

    let assign = client
        .patch(format!("{}/v1/tasks/{}", server.http_base, task_id))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "assignee_id": owner.user_id }))
        .send()
        .await
        .expect("assign task should return a response");
    assert_eq!(assign.status(), reqwest::StatusCode::OK);

    // Creating a comment requires an idempotency key.
    let missing_key = client
        .post(format!(
            "{}/v1/tasks/{}/comments",
            server.http_base, task_id
        ))
        .bearer_auth(&member_access)
        .json(&json!({ "body": "no key" }))
        .send()
        .await
        .expect("missing key comment should return a response");
    assert_eq!(missing_key.status(), reqwest::StatusCode::BAD_REQUEST);

    // Empty bodies are rejected.
    let empty_body = client
        .post(format!(
            "{}/v1/tasks/{}/comments",
            server.http_base, task_id
        ))
        .bearer_auth(&member_access)
        .header(
            "Idempotency-Key",
            format!("comment-{}", uuid::Uuid::new_v4()),
        )
        .json(&json!({ "body": "   " }))
        .send()
        .await
        .expect("empty comment should return a response");
    assert_eq!(empty_body.status(), reqwest::StatusCode::BAD_REQUEST);

    // Member comments; replaying the same key returns the same comment.
    let comment_key = format!("comment-{}", uuid::Uuid::new_v4());
    let mut member_comment_ids = Vec::new();
    for _ in 0..2 {
        let response = client
            .post(format!(
                "{}/v1/tasks/{}/comments",
                server.http_base, task_id
            ))
            .bearer_auth(&member_access)
            .header("Idempotency-Key", &comment_key)
            .json(&json!({ "body": "  Needs review  " }))
            .send()
            .await
            .expect("comment create should return a response");
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        let body: Value = response.json().await.expect("comment should be json");
        assert_eq!(body["body"], "Needs review", "body should be trimmed");
        assert_eq!(body["author_id"], member.user_id.as_str());
        member_comment_ids.push(body["id"].as_str().expect("comment id").to_string());
    }
    assert_eq!(
        member_comment_ids[0], member_comment_ids[1],
        "same key should replay the comment"
    );
    let member_comment_id = member_comment_ids[0].clone();

    // The assignee (owner) is notified about the member's comment.
    assert_eq!(
        count_notifications(&owner.email, "task_commented").await,
        1,
        "assignee should receive a comment notification"
    );

    // The owner commenting on their own assigned task does not notify anyone.
    let owner_comment: Value = client
        .post(format!(
            "{}/v1/tasks/{}/comments",
            server.http_base, task_id
        ))
        .bearer_auth(&owner.access_token)
        .header(
            "Idempotency-Key",
            format!("comment-{}", uuid::Uuid::new_v4()),
        )
        .json(&json!({ "body": "Owner reply" }))
        .send()
        .await
        .expect("owner comment should return a response")
        .json()
        .await
        .expect("owner comment should be json");
    let owner_comment_id = owner_comment["id"]
        .as_str()
        .expect("owner comment id")
        .to_string();
    assert_eq!(
        count_notifications(&owner.email, "task_commented").await,
        1,
        "self-comments should not enqueue notifications"
    );

    // Newest-first listing with keyset pagination.
    let first_page: Value = client
        .get(format!(
            "{}/v1/tasks/{}/comments?limit=1",
            server.http_base, task_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("comment list should return a response")
        .json()
        .await
        .expect("comment list should be json");
    let first_items = first_page["data"].as_array().expect("comment page array");
    assert_eq!(first_items.len(), 1);
    assert_eq!(
        first_items[0]["id"],
        owner_comment_id.as_str(),
        "newest comment should come first"
    );
    let cursor = first_page["next_cursor"]
        .as_str()
        .expect("first page should include a cursor")
        .to_string();

    let second_page: Value = client
        .get(format!(
            "{}/v1/tasks/{}/comments?limit=1&cursor={}",
            server.http_base, task_id, cursor
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("second comment page should return a response")
        .json()
        .await
        .expect("second comment page should be json");
    let second_items = second_page["data"].as_array().expect("comment page array");
    assert_eq!(second_items.len(), 1);
    assert_eq!(second_items[0]["id"], member_comment_id.as_str());
    assert!(
        second_page["next_cursor"].is_null(),
        "final page should not include a cursor"
    );

    // Only the author can edit a comment.
    let owner_edit = client
        .patch(format!(
            "{}/v1/tasks/{}/comments/{}",
            server.http_base, task_id, member_comment_id
        ))
        .bearer_auth(&owner.access_token)
        .json(&json!({ "body": "hijacked" }))
        .send()
        .await
        .expect("non-author edit should return a response");
    assert_eq!(owner_edit.status(), reqwest::StatusCode::FORBIDDEN);

    let member_edit: Value = client
        .patch(format!(
            "{}/v1/tasks/{}/comments/{}",
            server.http_base, task_id, member_comment_id
        ))
        .bearer_auth(&member_access)
        .json(&json!({ "body": "Needs review by Friday" }))
        .send()
        .await
        .expect("author edit should return a response")
        .json()
        .await
        .expect("author edit should be json");
    assert_eq!(member_edit["body"], "Needs review by Friday");

    // A member cannot delete someone else's comment.
    let member_delete = client
        .delete(format!(
            "{}/v1/tasks/{}/comments/{}",
            server.http_base, task_id, owner_comment_id
        ))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("member delete should return a response");
    assert_eq!(member_delete.status(), reqwest::StatusCode::FORBIDDEN);

    // The owner can moderate (delete) the member's comment.
    let owner_delete = client
        .delete(format!(
            "{}/v1/tasks/{}/comments/{}",
            server.http_base, task_id, member_comment_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("owner delete should return a response");
    assert_eq!(owner_delete.status(), reqwest::StatusCode::NO_CONTENT);

    let remaining: Value = client
        .get(format!(
            "{}/v1/tasks/{}/comments",
            server.http_base, task_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("final comment list should return a response")
        .json()
        .await
        .expect("final comment list should be json");
    let remaining_ids: Vec<&str> = remaining["data"]
        .as_array()
        .expect("comment array")
        .iter()
        .filter_map(|comment| comment["id"].as_str())
        .collect();
    assert_eq!(remaining_ids, vec![owner_comment_id.as_str()]);

    // Comment lifecycle shows up in the task audit feed.
    let audit: Value = client
        .get(format!(
            "{}/v1/tasks/{}/audit?limit=10",
            server.http_base, task_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task audit should return a response")
        .json()
        .await
        .expect("task audit should be json");
    let audit_events: Vec<&str> = audit["data"]
        .as_array()
        .expect("audit array")
        .iter()
        .filter_map(|entry| entry["event_type"].as_str())
        .collect();
    assert_eq!(
        audit_events[0], "task_comment_deleted",
        "delete should be the newest audit event"
    );
    assert!(
        audit_events.contains(&"task_comment_added"),
        "audit feed should include comment additions"
    );

    // Comments on unknown tasks return 404.
    let unknown = client
        .get(format!(
            "{}/v1/tasks/{}/comments",
            server.http_base,
            uuid::Uuid::new_v4()
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("unknown task comments should return a response");
    assert_eq!(unknown.status(), reqwest::StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires local Postgres and Redis services"]
async fn task_attachments_upload_download_and_delete() {
    let _guard = stack_test_guard().await;
    let server = TestServer::start().await;
    let client = Client::new();
    let owner = register_user(&client, &server.http_base, "attachments-owner").await;
    let member = register_user(&client, &server.http_base, "attachments-member").await;

    add_membership(&member.user_id, &owner.tenant_id, "member").await;
    let switched: Value = client
        .post(format!("{}/v1/auth/switch-tenant", server.http_base))
        .bearer_auth(&member.access_token)
        .json(&json!({ "tenant_id": owner.tenant_id }))
        .send()
        .await
        .expect("switch tenant should return a response")
        .json()
        .await
        .expect("switch tenant response should be json");
    let member_access = switched["access_token"]
        .as_str()
        .expect("switch should include access token")
        .to_string();

    let task = create_task(
        &client,
        &server.http_base,
        &owner.access_token,
        None,
        "Attachment target",
        "open",
        "medium",
    )
    .await;
    let task_id = task["id"].as_str().expect("task id").to_string();
    let attachments_url = format!("{}/v1/tasks/{}/attachments", server.http_base, task_id);

    // Uploads require an idempotency key.
    let missing_key = client
        .post(format!("{attachments_url}?file_name=notes.txt"))
        .bearer_auth(&member_access)
        .header("content-type", "text/plain")
        .body("no key")
        .send()
        .await
        .expect("missing key upload should return a response");
    assert_eq!(missing_key.status(), reqwest::StatusCode::BAD_REQUEST);

    // Empty bodies are rejected.
    let empty_body = client
        .post(format!("{attachments_url}?file_name=notes.txt"))
        .bearer_auth(&member_access)
        .header(
            "Idempotency-Key",
            format!("attachment-{}", uuid::Uuid::new_v4()),
        )
        .send()
        .await
        .expect("empty upload should return a response");
    assert_eq!(empty_body.status(), reqwest::StatusCode::BAD_REQUEST);

    // Unsafe file names are rejected.
    let bad_name = client
        .post(format!("{attachments_url}?file_name=a%2Fb.txt"))
        .bearer_auth(&member_access)
        .header(
            "Idempotency-Key",
            format!("attachment-{}", uuid::Uuid::new_v4()),
        )
        .body("content")
        .send()
        .await
        .expect("bad file name upload should return a response");
    assert_eq!(bad_name.status(), reqwest::StatusCode::BAD_REQUEST);

    // Oversized uploads are rejected (test server caps at 1024 bytes).
    let oversize = client
        .post(format!("{attachments_url}?file_name=big.bin"))
        .bearer_auth(&member_access)
        .header(
            "Idempotency-Key",
            format!("attachment-{}", uuid::Uuid::new_v4()),
        )
        .body(vec![0u8; 4096])
        .send()
        .await
        .expect("oversize upload should return a response");
    assert_eq!(oversize.status(), reqwest::StatusCode::PAYLOAD_TOO_LARGE);

    // Member uploads; replaying the same key returns the same attachment.
    let attachment_body = "hello attachment content";
    let upload_key = format!("attachment-{}", uuid::Uuid::new_v4());
    let mut member_attachment_ids = Vec::new();
    for _ in 0..2 {
        let response = client
            .post(format!("{attachments_url}?file_name=notes.txt"))
            .bearer_auth(&member_access)
            .header("Idempotency-Key", &upload_key)
            .header("content-type", "text/plain")
            .body(attachment_body)
            .send()
            .await
            .expect("upload should return a response");
        assert_eq!(response.status(), reqwest::StatusCode::CREATED);
        let body: Value = response.json().await.expect("attachment should be json");
        assert_eq!(body["file_name"], "notes.txt");
        assert_eq!(body["content_type"], "text/plain");
        assert_eq!(body["size_bytes"], attachment_body.len() as i64);
        assert_eq!(body["uploaded_by"], member.user_id.as_str());
        member_attachment_ids.push(body["id"].as_str().expect("attachment id").to_string());
    }
    assert_eq!(
        member_attachment_ids[0], member_attachment_ids[1],
        "same key should replay the attachment"
    );
    let member_attachment_id = member_attachment_ids[0].clone();

    // Download returns the original bytes with stored metadata.
    let download = client
        .get(format!("{attachments_url}/{member_attachment_id}/download"))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("download should return a response");
    assert_eq!(download.status(), reqwest::StatusCode::OK);
    assert_eq!(
        download
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("text/plain")
    );
    let disposition = download
        .headers()
        .get("content-disposition")
        .and_then(|value| value.to_str().ok())
        .expect("download should include content disposition")
        .to_string();
    assert!(disposition.contains("notes.txt"));
    let downloaded = download.text().await.expect("download body should read");
    assert_eq!(downloaded, attachment_body);

    // Owner uploads a second attachment.
    let owner_attachment: Value = client
        .post(format!("{attachments_url}?file_name=spec.md"))
        .bearer_auth(&owner.access_token)
        .header(
            "Idempotency-Key",
            format!("attachment-{}", uuid::Uuid::new_v4()),
        )
        .header("content-type", "text/markdown")
        .body("# spec")
        .send()
        .await
        .expect("owner upload should return a response")
        .json()
        .await
        .expect("owner upload should be json");
    let owner_attachment_id = owner_attachment["id"]
        .as_str()
        .expect("owner attachment id")
        .to_string();

    // Listing is newest first and includes a download path.
    let listed: Value = client
        .get(&attachments_url)
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("attachment list should return a response")
        .json()
        .await
        .expect("attachment list should be json");
    let items = listed.as_array().expect("attachment array");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], owner_attachment_id.as_str());
    assert_eq!(items[1]["id"], member_attachment_id.as_str());
    assert_eq!(
        items[1]["download_path"],
        format!("/v1/tasks/{task_id}/attachments/{member_attachment_id}/download")
    );

    // A member cannot delete someone else's attachment.
    let member_delete = client
        .delete(format!("{attachments_url}/{owner_attachment_id}"))
        .bearer_auth(&member_access)
        .send()
        .await
        .expect("member delete should return a response");
    assert_eq!(member_delete.status(), reqwest::StatusCode::FORBIDDEN);

    // The owner can moderate (delete) the member's attachment.
    let owner_delete = client
        .delete(format!("{attachments_url}/{member_attachment_id}"))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("owner delete should return a response");
    assert_eq!(owner_delete.status(), reqwest::StatusCode::NO_CONTENT);

    // Deleted attachments are gone from listing and download.
    let after_delete = client
        .get(format!("{attachments_url}/{member_attachment_id}/download"))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("deleted download should return a response");
    assert_eq!(after_delete.status(), reqwest::StatusCode::NOT_FOUND);

    let remaining: Value = client
        .get(&attachments_url)
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("final attachment list should return a response")
        .json()
        .await
        .expect("final attachment list should be json");
    let remaining_ids: Vec<&str> = remaining
        .as_array()
        .expect("attachment array")
        .iter()
        .filter_map(|attachment| attachment["id"].as_str())
        .collect();
    assert_eq!(remaining_ids, vec![owner_attachment_id.as_str()]);

    // Attachment lifecycle shows up in the task audit feed.
    let audit: Value = client
        .get(format!(
            "{}/v1/tasks/{}/audit?limit=10",
            server.http_base, task_id
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("task audit should return a response")
        .json()
        .await
        .expect("task audit should be json");
    let audit_events: Vec<&str> = audit["data"]
        .as_array()
        .expect("audit array")
        .iter()
        .filter_map(|entry| entry["event_type"].as_str())
        .collect();
    assert_eq!(
        audit_events[0], "task_attachment_deleted",
        "delete should be the newest audit event"
    );
    assert!(
        audit_events.contains(&"task_attachment_added"),
        "audit feed should include attachment uploads"
    );

    // Attachments on unknown tasks return 404.
    let unknown = client
        .get(format!(
            "{}/v1/tasks/{}/attachments",
            server.http_base,
            uuid::Uuid::new_v4()
        ))
        .bearer_auth(&owner.access_token)
        .send()
        .await
        .expect("unknown task attachments should return a response");
    assert_eq!(unknown.status(), reqwest::StatusCode::NOT_FOUND);
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
