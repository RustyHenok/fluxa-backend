use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::watch;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{Request, Response, Status, transport::Server};
use tracing::info;
use uuid::Uuid;

use crate::domain::{BackgroundJobRecord, TaskFilters, TaskRecord};
use crate::error::{AppError, AppResult};
use crate::pagination::Cursor;
use crate::services::{jobs as jobs_service, tasks as task_service};
use crate::state::AppState;

pub mod proto {
    tonic::include_proto!("fluxa.internal.v1");
}

use proto::job_admin_server::{JobAdmin, JobAdminServer};
use proto::task_read_server::{TaskRead, TaskReadServer};
use proto::{
    EnqueueExportRequest, GetJobStatusRequest, GetTaskSnapshotRequest, JobReply,
    ListTaskSummariesRequest, RunDueReminderSweepRequest, TaskListReply, TaskReply,
};

pub async fn serve(state: AppState, mut shutdown: watch::Receiver<bool>) -> AppResult<()> {
    let listener = crate::bind_listener(state.config.grpc_addr).await?;
    let incoming = TcpListenerStream::new(listener);
    info!("grpc server listening on {}", state.config.grpc_addr);

    Server::builder()
        .add_service(JobAdminServer::new(JobAdminService {
            state: state.clone(),
        }))
        .add_service(TaskReadServer::new(TaskReadService { state }))
        .serve_with_incoming_shutdown(incoming, async move {
            let _ = shutdown.changed().await;
        })
        .await
        .map_err(|error| AppError::internal(format!("grpc server failed: {error}")))
}

#[derive(Clone)]
struct JobAdminService {
    state: AppState,
}

#[derive(Clone)]
struct TaskReadService {
    state: AppState,
}

#[tonic::async_trait]
impl JobAdmin for JobAdminService {
    async fn enqueue_export(
        &self,
        request: Request<EnqueueExportRequest>,
    ) -> Result<Response<JobReply>, Status> {
        let payload = request.into_inner();
        let tenant_id = parse_uuid(&payload.tenant_id, "tenant_id")?;
        let requested_by = parse_uuid(&payload.requested_by, "requested_by")?;
        let filters = filters_from_parts(
            payload.status,
            payload.priority,
            option_uuid(payload.assignee_id)?,
            option_datetime(payload.due_before)?,
            option_datetime(payload.due_after)?,
            option_datetime(payload.updated_after)?,
            option_string(payload.q),
        )?;

        let job = jobs_service::create_export_job(&self.state, tenant_id, requested_by, &filters)
            .await
            .map_err(status_from_error)?;

        Ok(Response::new(job_to_proto(&job)))
    }

    async fn run_due_reminder_sweep(
        &self,
        request: Request<RunDueReminderSweepRequest>,
    ) -> Result<Response<JobReply>, Status> {
        let payload = request.into_inner();
        let tenant_id = option_uuid(payload.tenant_id)?;
        let maybe_job = jobs_service::enqueue_due_reminder_sweep(&self.state, tenant_id)
            .await
            .map_err(status_from_error)?;

        let job = match maybe_job {
            Some(job) => job,
            None => {
                return Err(Status::already_exists(
                    "a due reminder sweep is already queued or running",
                ));
            }
        };
        Ok(Response::new(job_to_proto(&job)))
    }

    async fn get_job_status(
        &self,
        request: Request<GetJobStatusRequest>,
    ) -> Result<Response<JobReply>, Status> {
        let job_id = parse_uuid(&request.into_inner().job_id, "job_id")?;
        let job = jobs_service::get_job(&self.state, job_id)
            .await
            .map_err(status_from_error)?
            .ok_or_else(|| Status::not_found("job not found"))?;

        Ok(Response::new(job_to_proto(&job)))
    }
}

#[tonic::async_trait]
impl TaskRead for TaskReadService {
    async fn get_task_snapshot(
        &self,
        request: Request<GetTaskSnapshotRequest>,
    ) -> Result<Response<TaskReply>, Status> {
        let payload = request.into_inner();
        let tenant_id = parse_uuid(&payload.tenant_id, "tenant_id")?;
        let task_id = parse_uuid(&payload.task_id, "task_id")?;
        let task = task_service::get_task(&self.state, tenant_id, task_id)
            .await
            .map_err(status_from_error)?;

        Ok(Response::new(task_to_proto(&task)))
    }

    async fn list_task_summaries(
        &self,
        request: Request<ListTaskSummariesRequest>,
    ) -> Result<Response<TaskListReply>, Status> {
        let payload = request.into_inner();
        let tenant_id = parse_uuid(&payload.tenant_id, "tenant_id")?;
        let limit = payload.limit.clamp(1, 100) as usize;
        let cursor = option_string(payload.cursor)
            .map(|value| Cursor::decode(&value))
            .transpose()
            .map_err(status_from_error)?;
        let filters = filters_from_parts(
            payload.status,
            payload.priority,
            option_uuid(payload.assignee_id)?,
            option_datetime(payload.due_before)?,
            option_datetime(payload.due_after)?,
            option_datetime(payload.updated_after)?,
            option_string(payload.q),
        )?;

        let result =
            task_service::list_tasks(&self.state, tenant_id, &filters, cursor.as_ref(), limit)
                .await
                .map_err(status_from_error)?;
        let next_cursor = result
            .next_cursor
            .map(|cursor| cursor.encode())
            .transpose()
            .map_err(status_from_error)?
            .unwrap_or_default();

        Ok(Response::new(TaskListReply {
            tasks: result.tasks.iter().map(task_to_proto).collect(),
            next_cursor,
        }))
    }
}

fn filters_from_parts(
    status: String,
    priority: String,
    assignee_id: Option<Uuid>,
    due_before: Option<DateTime<Utc>>,
    due_after: Option<DateTime<Utc>>,
    updated_after: Option<DateTime<Utc>>,
    q: Option<String>,
) -> Result<TaskFilters, Status> {
    TaskFilters {
        status: option_string(status).map(|value| value.to_ascii_lowercase()),
        priority: option_string(priority).map(|value| value.to_ascii_lowercase()),
        assignee_id,
        due_before,
        due_after,
        updated_after,
        q,
    }
    .validate()
    .map_err(status_from_error)
}

fn parse_uuid(value: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(value)
        .map_err(|error| Status::invalid_argument(format!("invalid {field}: {error}")))
}

fn option_uuid(value: String) -> Result<Option<Uuid>, Status> {
    let value = option_string(value);
    value.map(|value| parse_uuid(&value, "uuid")).transpose()
}

fn option_string(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn option_datetime(value: String) -> Result<Option<DateTime<Utc>>, Status> {
    let value = match option_string(value) {
        Some(value) => value,
        None => return Ok(None),
    };

    DateTime::parse_from_rfc3339(&value)
        .map(|value| Some(value.with_timezone(&Utc)))
        .map_err(|error| Status::invalid_argument(format!("invalid datetime: {error}")))
}

fn status_from_error(error: AppError) -> Status {
    match error {
        AppError::Validation(message) => Status::invalid_argument(message),
        AppError::Unauthorized(message) => Status::unauthenticated(message),
        AppError::Forbidden(message) => Status::permission_denied(message),
        AppError::NotFound(message) => Status::not_found(message),
        AppError::Conflict(message) => Status::already_exists(message),
        AppError::RateLimited(message) => Status::resource_exhausted(message),
        AppError::Internal(message) => Status::internal(message),
    }
}

fn task_to_proto(task: &TaskRecord) -> TaskReply {
    TaskReply {
        id: task.id.to_string(),
        tenant_id: task.tenant_id.to_string(),
        title: task.title.clone(),
        description: task.description.clone().unwrap_or_default(),
        status: task.status.clone(),
        priority: task.priority.clone(),
        assignee_id: task
            .assignee_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        created_by: task.created_by.to_string(),
        updated_by: task.updated_by.to_string(),
        due_at: task.due_at.map(timestamp),
        created_at: Some(timestamp(task.created_at)),
        updated_at: Some(timestamp(task.updated_at)),
    }
}

fn job_to_proto(job: &BackgroundJobRecord) -> JobReply {
    JobReply {
        job_id: job.id.to_string(),
        tenant_id: job
            .tenant_id
            .map(|value| value.to_string())
            .unwrap_or_default(),
        job_type: job.job_type.clone(),
        status: job.status.clone(),
        attempts: job.attempts as u32,
        max_attempts: job.max_attempts as u32,
        last_error: job.last_error.clone().unwrap_or_default(),
        payload: Some(struct_from_json(job.payload.clone())),
        result_payload: Some(struct_from_json(
            job.result_payload.clone().unwrap_or_else(|| json!({})),
        )),
        scheduled_at: Some(timestamp(job.scheduled_at)),
        started_at: job.started_at.map(timestamp),
        finished_at: job.finished_at.map(timestamp),
    }
}

fn timestamp(value: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: value.timestamp(),
        nanos: value.timestamp_subsec_nanos() as i32,
    }
}

fn struct_from_json(value: serde_json::Value) -> prost_types::Struct {
    match value {
        serde_json::Value::Object(map) => prost_types::Struct {
            fields: map
                .into_iter()
                .map(|(key, value)| (key, json_value(value)))
                .collect(),
        },
        other => prost_types::Struct {
            fields: BTreeMap::from([("value".into(), json_value(other))]),
        },
    }
}

fn json_value(value: serde_json::Value) -> prost_types::Value {
    prost_types::Value {
        kind: Some(match value {
            serde_json::Value::Null => prost_types::value::Kind::NullValue(0),
            serde_json::Value::Bool(value) => prost_types::value::Kind::BoolValue(value),
            serde_json::Value::Number(value) => {
                prost_types::value::Kind::NumberValue(value.as_f64().unwrap_or_default())
            }
            serde_json::Value::String(value) => prost_types::value::Kind::StringValue(value),
            serde_json::Value::Array(values) => {
                prost_types::value::Kind::ListValue(prost_types::ListValue {
                    values: values.into_iter().map(json_value).collect(),
                })
            }
            serde_json::Value::Object(map) => {
                prost_types::value::Kind::StructValue(prost_types::Struct {
                    fields: map
                        .into_iter()
                        .map(|(key, value)| (key, json_value(value)))
                        .collect(),
                })
            }
        }),
    }
}
