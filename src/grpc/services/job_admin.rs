use tonic::{Request, Response, Status};

use crate::services::jobs as jobs_service;
use crate::state::AppState;

use super::super::mapping::job_to_proto;
use super::super::parsing::{
    filters_from_parts, option_datetime, option_string, option_uuid, parse_uuid, status_from_error,
};
use super::super::proto::job_admin_server::JobAdmin;
use super::super::proto::{
    EnqueueExportRequest, GetJobStatusRequest, JobReply, RunDueReminderSweepRequest,
};

#[derive(Clone)]
pub(crate) struct JobAdminService {
    state: AppState,
}

impl JobAdminService {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
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
