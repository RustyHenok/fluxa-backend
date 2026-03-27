use tonic::{Request, Response, Status};

use crate::pagination::Cursor;
use crate::services::tasks as task_service;
use crate::state::AppState;

use super::super::mapping::task_to_proto;
use super::super::parsing::{
    filters_from_parts, option_datetime, option_string, option_uuid, parse_uuid, status_from_error,
};
use super::super::proto::task_read_server::TaskRead;
use super::super::proto::{
    GetTaskSnapshotRequest, ListTaskSummariesRequest, TaskListReply, TaskReply,
};

#[derive(Clone)]
pub(crate) struct TaskReadService {
    state: AppState,
}

impl TaskReadService {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
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
