use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::json;

use crate::domain::{BackgroundJobRecord, TaskRecord};

use super::proto::{JobReply, TaskReply};

pub(super) fn task_to_proto(task: &TaskRecord) -> TaskReply {
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

pub(super) fn job_to_proto(job: &BackgroundJobRecord) -> JobReply {
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
