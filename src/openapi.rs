use serde_json::{Value, json};

use crate::error::{AppError, AppResult};

pub fn document() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Fluxa Backend API",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Multi-tenant task platform REST API used by the Fluxa web and mobile clients.",
        },
        "servers": [
            {
                "url": "http://127.0.0.1:18080",
                "description": "Local Docker Compose stack",
            }
        ],
        "tags": [
            { "name": "system", "description": "Operational health and metrics endpoints." },
            { "name": "auth", "description": "Authentication and tenant session endpoints." },
            { "name": "tenants", "description": "Tenant-scoped membership endpoints." },
            { "name": "tasks", "description": "Task CRUD, filtering, and audit endpoints." },
            { "name": "jobs", "description": "Background job creation, status, and results." }
        ],
        "security": [
            { "bearerAuth": [] }
        ],
        "paths": {
            "/healthz": {
                "get": {
                    "tags": ["system"],
                    "operationId": "getHealth",
                    "summary": "Health check",
                    "security": [],
                    "responses": {
                        "200": json_response("Service is alive.", schema_ref("HealthResponse"))
                    }
                }
            },
            "/readyz": {
                "get": {
                    "tags": ["system"],
                    "operationId": "getReadiness",
                    "summary": "Readiness check",
                    "security": [],
                    "responses": {
                        "200": json_response("Service dependencies are ready.", schema_ref("HealthResponse")),
                        "500": error_response("A dependency is unavailable.")
                    }
                }
            },
            "/metrics": {
                "get": {
                    "tags": ["system"],
                    "operationId": "getMetrics",
                    "summary": "Prometheus metrics",
                    "security": [],
                    "responses": {
                        "200": text_response("Prometheus metrics in text exposition format.")
                    }
                }
            },
            "/v1/auth/register": {
                "post": {
                    "tags": ["auth"],
                    "operationId": "register",
                    "summary": "Register a user and bootstrap a tenant",
                    "security": [],
                    "requestBody": json_request_body(schema_ref("RegisterRequest"), true),
                    "responses": {
                        "201": json_response("Registration succeeded.", schema_ref("AuthResponse")),
                        "400": error_response("Invalid registration payload."),
                        "409": error_response("A resource already exists."),
                        "429": error_response("Too many registration attempts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/auth/login": {
                "post": {
                    "tags": ["auth"],
                    "operationId": "login",
                    "summary": "Authenticate a user",
                    "security": [],
                    "requestBody": json_request_body(schema_ref("LoginRequest"), true),
                    "responses": {
                        "200": json_response("Login succeeded.", schema_ref("AuthResponse")),
                        "400": error_response("Invalid login payload."),
                        "401": error_response("Credentials were rejected."),
                        "429": error_response("Too many login attempts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/auth/refresh": {
                "post": {
                    "tags": ["auth"],
                    "operationId": "refreshSession",
                    "summary": "Rotate access and refresh tokens",
                    "security": [],
                    "requestBody": json_request_body(schema_ref("RefreshRequest"), true),
                    "responses": {
                        "200": json_response("Session refresh succeeded.", schema_ref("AuthResponse")),
                        "400": error_response("Invalid refresh payload."),
                        "401": error_response("Refresh token was invalid or revoked."),
                        "429": error_response("Too many refresh attempts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/auth/logout": {
                "post": {
                    "tags": ["auth"],
                    "operationId": "logout",
                    "summary": "Revoke a refresh token",
                    "security": [],
                    "requestBody": json_request_body(schema_ref("LogoutRequest"), true),
                    "responses": {
                        "204": no_content_response("Refresh token revoked."),
                        "400": error_response("Invalid logout payload."),
                        "401": error_response("Refresh token was invalid."),
                        "429": error_response("Too many logout attempts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/auth/switch-tenant": {
                "post": {
                    "tags": ["auth"],
                    "operationId": "switchTenant",
                    "summary": "Issue a new session scoped to a different tenant",
                    "requestBody": json_request_body(schema_ref("SwitchTenantRequest"), true),
                    "responses": {
                        "200": json_response("Tenant switch succeeded.", schema_ref("AuthResponse")),
                        "400": error_response("Invalid tenant switch payload."),
                        "401": error_response("Authentication is required."),
                        "404": error_response("Tenant membership was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/me": {
                "get": {
                    "tags": ["auth"],
                    "operationId": "getCurrentUser",
                    "summary": "Get the active user profile",
                    "responses": {
                        "200": json_response("Current user profile.", schema_ref("MeResponse")),
                        "401": error_response("Authentication is required."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/me/tenants": {
                "get": {
                    "tags": ["auth"],
                    "operationId": "listCurrentUserTenants",
                    "summary": "List the current user's tenant memberships",
                    "responses": {
                        "200": {
                            "description": "Tenant memberships for the authenticated user.",
                            "content": {
                                "application/json": {
                                    "schema": array_schema(schema_ref("TenantMembershipResponse"))
                                }
                            }
                        },
                        "401": error_response("Authentication is required."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/dashboard/summary": {
                "get": {
                    "tags": ["tasks"],
                    "operationId": "getDashboardSummary",
                    "summary": "Get tenant task summary counts",
                    "responses": {
                        "200": json_response("Tenant summary counts.", schema_ref("DashboardSummary")),
                        "401": error_response("Authentication is required."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/tenants/{tenant_id}/members": {
                "get": {
                    "tags": ["tenants"],
                    "operationId": "listTenantMembers",
                    "summary": "List members of the active tenant",
                    "parameters": [
                        path_uuid_parameter("tenant_id", "Tenant identifier.")
                    ],
                    "responses": {
                        "200": {
                            "description": "Tenant members.",
                            "content": {
                                "application/json": {
                                    "schema": array_schema(schema_ref("TenantMemberResponse"))
                                }
                            }
                        },
                        "401": error_response("Authentication is required."),
                        "404": error_response("Tenant was not found for the active membership."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/tasks": {
                "get": {
                    "tags": ["tasks"],
                    "operationId": "listTasks",
                    "summary": "List tasks with cursor pagination and filters",
                    "parameters": [
                        limit_query_parameter(),
                        cursor_query_parameter("cursor", "Opaque cursor from a previous task page."),
                        query_parameter("status", false, "Filter by task status.", schema_ref("TaskStatus")),
                        query_parameter("priority", false, "Filter by task priority.", schema_ref("TaskPriority")),
                        query_parameter("assignee_id", false, "Filter by assignee.", uuid_schema()),
                        query_parameter("due_before", false, "Return tasks due before this RFC3339 timestamp.", date_time_schema()),
                        query_parameter("due_after", false, "Return tasks due after this RFC3339 timestamp.", date_time_schema()),
                        query_parameter("updated_after", false, "Return tasks updated after this RFC3339 timestamp.", date_time_schema()),
                        query_parameter("q", false, "Full-text search term applied to the task title and description.", string_schema())
                    ],
                    "responses": {
                        "200": json_response("Paginated task list.", schema_ref("TaskListResponse")),
                        "400": error_response("Invalid query parameters."),
                        "401": error_response("Authentication is required."),
                        "500": error_response("Unexpected server error.")
                    }
                },
                "post": {
                    "tags": ["tasks"],
                    "operationId": "createTask",
                    "summary": "Create a task",
                    "parameters": [
                        idempotency_header_parameter()
                    ],
                    "requestBody": json_request_body(schema_ref("TaskPayload"), true),
                    "responses": {
                        "201": json_response("Task created.", schema_ref("TaskResponse")),
                        "400": error_response("Invalid task payload."),
                        "401": error_response("Authentication is required."),
                        "403": error_response("The active role cannot create tasks."),
                        "409": error_response("The idempotency key is in progress or conflicts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/tasks/{task_id}": {
                "get": {
                    "tags": ["tasks"],
                    "operationId": "getTask",
                    "summary": "Get a single task",
                    "parameters": [
                        path_uuid_parameter("task_id", "Task identifier.")
                    ],
                    "responses": {
                        "200": json_response("Task detail.", schema_ref("TaskResponse")),
                        "401": error_response("Authentication is required."),
                        "404": error_response("Task was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                },
                "patch": {
                    "tags": ["tasks"],
                    "operationId": "updateTask",
                    "summary": "Update a task",
                    "parameters": [
                        path_uuid_parameter("task_id", "Task identifier.")
                    ],
                    "requestBody": json_request_body(schema_ref("TaskPatchPayload"), true),
                    "responses": {
                        "200": json_response("Updated task detail.", schema_ref("TaskResponse")),
                        "400": error_response("Invalid task patch payload."),
                        "401": error_response("Authentication is required."),
                        "403": error_response("The active role cannot update tasks."),
                        "404": error_response("Task was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                },
                "delete": {
                    "tags": ["tasks"],
                    "operationId": "deleteTask",
                    "summary": "Delete a task",
                    "parameters": [
                        path_uuid_parameter("task_id", "Task identifier.")
                    ],
                    "responses": {
                        "204": no_content_response("Task deleted."),
                        "401": error_response("Authentication is required."),
                        "403": error_response("The active role cannot delete tasks."),
                        "404": error_response("Task was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/tasks/{task_id}/audit": {
                "get": {
                    "tags": ["tasks"],
                    "operationId": "listTaskAudit",
                    "summary": "List task audit events",
                    "parameters": [
                        path_uuid_parameter("task_id", "Task identifier."),
                        limit_query_parameter(),
                        cursor_query_parameter("cursor", "Opaque cursor from a previous task audit page.")
                    ],
                    "responses": {
                        "200": json_response("Paginated task audit events.", schema_ref("TaskAuditListResponse")),
                        "400": error_response("Invalid query parameters."),
                        "401": error_response("Authentication is required."),
                        "404": error_response("Task was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/exports/tasks": {
                "post": {
                    "tags": ["jobs"],
                    "operationId": "createTaskExport",
                    "summary": "Create a task export job",
                    "parameters": [
                        idempotency_header_parameter()
                    ],
                    "requestBody": json_request_body(schema_ref("ExportRequest"), true),
                    "responses": {
                        "202": json_response("Export job accepted.", schema_ref("JobResponse")),
                        "400": error_response("Invalid export payload."),
                        "401": error_response("Authentication is required."),
                        "403": error_response("The active role cannot create export jobs."),
                        "409": error_response("The idempotency key is in progress or conflicts."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/jobs/{job_id}": {
                "get": {
                    "tags": ["jobs"],
                    "operationId": "getJob",
                    "summary": "Get a background job status",
                    "parameters": [
                        path_uuid_parameter("job_id", "Job identifier.")
                    ],
                    "responses": {
                        "200": json_response("Job status detail.", schema_ref("JobResponse")),
                        "401": error_response("Authentication is required."),
                        "404": error_response("Job was not found."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            },
            "/v1/jobs/{job_id}/result": {
                "get": {
                    "tags": ["jobs"],
                    "operationId": "getJobResult",
                    "summary": "Get the finalized job result payload",
                    "parameters": [
                        path_uuid_parameter("job_id", "Job identifier.")
                    ],
                    "responses": {
                        "200": json_response("Completed job result.", schema_ref("JobResultResponse")),
                        "401": error_response("Authentication is required."),
                        "404": error_response("Job was not found."),
                        "409": error_response("Job result is not available yet."),
                        "500": error_response("Unexpected server error.")
                    }
                }
            }
        },
        "components": {
            "securitySchemes": {
                "bearerAuth": {
                    "type": "http",
                    "scheme": "bearer",
                    "bearerFormat": "JWT"
                }
            },
            "schemas": {
                "ErrorEnvelope": {
                    "type": "object",
                    "required": ["error"],
                    "properties": {
                        "error": schema_ref("ErrorBody")
                    }
                },
                "ErrorBody": {
                    "type": "object",
                    "required": ["code", "message"],
                    "properties": {
                        "code": { "type": "string" },
                        "message": { "type": "string" }
                    }
                },
                "HealthResponse": {
                    "type": "object",
                    "required": ["status"],
                    "properties": {
                        "status": { "type": "string" }
                    }
                },
                "MembershipRole": {
                    "type": "string",
                    "enum": ["owner", "admin", "member"]
                },
                "TaskStatus": {
                    "type": "string",
                    "enum": ["open", "in_progress", "done", "archived"]
                },
                "TaskPriority": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "urgent"]
                },
                "JobStatus": {
                    "type": "string",
                    "enum": ["queued", "running", "completed", "dead_letter"]
                },
                "JobType": {
                    "type": "string",
                    "enum": ["task_export", "due_reminder_sweep"]
                },
                "UserResponse": {
                    "type": "object",
                    "required": ["id", "email", "created_at"],
                    "properties": {
                        "id": uuid_schema(),
                        "email": string_schema(),
                        "created_at": date_time_schema()
                    }
                },
                "TenantMembershipResponse": {
                    "type": "object",
                    "required": ["tenant_id", "tenant_name", "role", "created_at"],
                    "properties": {
                        "tenant_id": uuid_schema(),
                        "tenant_name": string_schema(),
                        "role": schema_ref("MembershipRole"),
                        "created_at": date_time_schema()
                    }
                },
                "TenantMemberResponse": {
                    "type": "object",
                    "required": ["user_id", "email", "role", "joined_at"],
                    "properties": {
                        "user_id": uuid_schema(),
                        "email": string_schema(),
                        "role": schema_ref("MembershipRole"),
                        "joined_at": date_time_schema()
                    }
                },
                "RegisterRequest": {
                    "type": "object",
                    "required": ["email", "password"],
                    "properties": {
                        "email": string_schema(),
                        "password": string_schema(),
                        "tenant_name": nullable(string_schema())
                    }
                },
                "LoginRequest": {
                    "type": "object",
                    "required": ["email", "password"],
                    "properties": {
                        "email": string_schema(),
                        "password": string_schema(),
                        "tenant_id": nullable(uuid_schema())
                    }
                },
                "RefreshRequest": {
                    "type": "object",
                    "required": ["refresh_token"],
                    "properties": {
                        "refresh_token": string_schema(),
                        "tenant_id": nullable(uuid_schema())
                    }
                },
                "LogoutRequest": {
                    "type": "object",
                    "required": ["refresh_token"],
                    "properties": {
                        "refresh_token": string_schema()
                    }
                },
                "SwitchTenantRequest": {
                    "type": "object",
                    "required": ["tenant_id"],
                    "properties": {
                        "tenant_id": uuid_schema()
                    }
                },
                "AuthResponse": {
                    "type": "object",
                    "required": [
                        "access_token",
                        "refresh_token",
                        "expires_in_seconds",
                        "user",
                        "active_tenant"
                    ],
                    "properties": {
                        "access_token": string_schema(),
                        "refresh_token": string_schema(),
                        "expires_in_seconds": int64_schema(),
                        "user": schema_ref("UserResponse"),
                        "active_tenant": schema_ref("TenantMembershipResponse")
                    }
                },
                "MeResponse": {
                    "type": "object",
                    "required": ["user", "active_tenant"],
                    "properties": {
                        "user": schema_ref("UserResponse"),
                        "active_tenant": schema_ref("TenantMembershipResponse")
                    }
                },
                "DashboardSummary": {
                    "type": "object",
                    "required": [
                        "open_task_count",
                        "in_progress_task_count",
                        "done_task_count",
                        "overdue_task_count",
                        "recent_activity_count"
                    ],
                    "properties": {
                        "open_task_count": int64_schema(),
                        "in_progress_task_count": int64_schema(),
                        "done_task_count": int64_schema(),
                        "overdue_task_count": int64_schema(),
                        "recent_activity_count": int64_schema()
                    }
                },
                "TaskPayload": {
                    "type": "object",
                    "required": ["title"],
                    "properties": {
                        "title": string_schema(),
                        "description": nullable(string_schema()),
                        "status": schema_ref("TaskStatus"),
                        "priority": schema_ref("TaskPriority"),
                        "assignee_id": nullable(uuid_schema()),
                        "due_at": nullable(date_time_schema())
                    }
                },
                "TaskPatchPayload": {
                    "type": "object",
                    "properties": {
                        "title": string_schema(),
                        "description": nullable(string_schema()),
                        "status": schema_ref("TaskStatus"),
                        "priority": schema_ref("TaskPriority"),
                        "assignee_id": nullable(uuid_schema()),
                        "due_at": nullable(date_time_schema())
                    }
                },
                "TaskResponse": {
                    "type": "object",
                    "required": [
                        "id",
                        "tenant_id",
                        "title",
                        "description",
                        "status",
                        "priority",
                        "assignee_id",
                        "due_at",
                        "created_by",
                        "updated_by",
                        "created_at",
                        "updated_at"
                    ],
                    "properties": {
                        "id": uuid_schema(),
                        "tenant_id": uuid_schema(),
                        "title": string_schema(),
                        "description": nullable(string_schema()),
                        "status": schema_ref("TaskStatus"),
                        "priority": schema_ref("TaskPriority"),
                        "assignee_id": nullable(uuid_schema()),
                        "due_at": nullable(date_time_schema()),
                        "created_by": uuid_schema(),
                        "updated_by": uuid_schema(),
                        "created_at": date_time_schema(),
                        "updated_at": date_time_schema()
                    }
                },
                "TaskListResponse": {
                    "type": "object",
                    "required": ["data", "next_cursor"],
                    "properties": {
                        "data": array_schema(schema_ref("TaskResponse")),
                        "next_cursor": nullable(string_schema())
                    }
                },
                "TaskAuditResponse": {
                    "type": "object",
                    "required": [
                        "id",
                        "task_id",
                        "tenant_id",
                        "actor_user_id",
                        "event_type",
                        "payload",
                        "created_at"
                    ],
                    "properties": {
                        "id": uuid_schema(),
                        "task_id": nullable(uuid_schema()),
                        "tenant_id": uuid_schema(),
                        "actor_user_id": uuid_schema(),
                        "event_type": string_schema(),
                        "payload": schema_ref("FreeformObject"),
                        "created_at": date_time_schema()
                    }
                },
                "TaskAuditListResponse": {
                    "type": "object",
                    "required": ["data", "next_cursor"],
                    "properties": {
                        "data": array_schema(schema_ref("TaskAuditResponse")),
                        "next_cursor": nullable(string_schema())
                    }
                },
                "ExportRequest": {
                    "type": "object",
                    "properties": {
                        "status": schema_ref("TaskStatus"),
                        "priority": schema_ref("TaskPriority"),
                        "assignee_id": uuid_schema(),
                        "due_before": date_time_schema(),
                        "due_after": date_time_schema(),
                        "updated_after": date_time_schema(),
                        "q": string_schema()
                    }
                },
                "TaskFilters": {
                    "type": "object",
                    "properties": {
                        "status": schema_ref("TaskStatus"),
                        "priority": schema_ref("TaskPriority"),
                        "assignee_id": uuid_schema(),
                        "due_before": date_time_schema(),
                        "due_after": date_time_schema(),
                        "updated_after": date_time_schema(),
                        "q": string_schema()
                    }
                },
                "TaskExportJobPayload": {
                    "type": "object",
                    "required": ["tenant_id", "requested_by", "filters"],
                    "properties": {
                        "tenant_id": uuid_schema(),
                        "requested_by": uuid_schema(),
                        "filters": schema_ref("TaskFilters")
                    }
                },
                "DueReminderSweepJobPayload": {
                    "type": "object",
                    "required": ["tenant_id"],
                    "properties": {
                        "tenant_id": nullable(uuid_schema())
                    }
                },
                "TaskExportJobResult": {
                    "type": "object",
                    "required": ["requested_by", "generated_at", "task_count", "tasks"],
                    "properties": {
                        "requested_by": uuid_schema(),
                        "generated_at": date_time_schema(),
                        "task_count": int64_schema(),
                        "tasks": array_schema(schema_ref("TaskResponse"))
                    }
                },
                "DueReminderSweepJobResult": {
                    "type": "object",
                    "required": ["generated_at", "tenant_id", "reminder_count"],
                    "properties": {
                        "generated_at": date_time_schema(),
                        "tenant_id": nullable(uuid_schema()),
                        "reminder_count": int64_schema()
                    }
                },
                "JobResponse": {
                    "type": "object",
                    "required": [
                        "id",
                        "tenant_id",
                        "job_type",
                        "status",
                        "attempts",
                        "max_attempts",
                        "scheduled_at",
                        "started_at",
                        "finished_at",
                        "payload",
                        "result_payload",
                        "last_error"
                    ],
                    "properties": {
                        "id": uuid_schema(),
                        "tenant_id": nullable(uuid_schema()),
                        "job_type": schema_ref("JobType"),
                        "status": schema_ref("JobStatus"),
                        "attempts": int32_schema(),
                        "max_attempts": int32_schema(),
                        "scheduled_at": date_time_schema(),
                        "started_at": nullable(date_time_schema()),
                        "finished_at": nullable(date_time_schema()),
                        "payload": job_payload_schema(),
                        "result_payload": nullable(job_result_payload_schema()),
                        "last_error": nullable(string_schema())
                    }
                },
                "JobResultResponse": {
                    "type": "object",
                    "required": ["job_id", "job_type", "finished_at", "result"],
                    "properties": {
                        "job_id": uuid_schema(),
                        "job_type": schema_ref("JobType"),
                        "finished_at": nullable(date_time_schema()),
                        "result": job_result_payload_schema()
                    }
                },
                "FreeformObject": {
                    "type": "object",
                    "additionalProperties": true
                }
            }
        }
    })
}

pub fn render_pretty() -> AppResult<String> {
    serde_json::to_string_pretty(&document())
        .map_err(|error| AppError::internal(format!("failed to render openapi document: {error}")))
}

fn schema_ref(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn json_request_body(schema: Value, required: bool) -> Value {
    json!({
        "required": required,
        "content": {
            "application/json": {
                "schema": schema
            }
        }
    })
}

fn json_response(description: &str, schema: Value) -> Value {
    json!({
        "description": description,
        "content": {
            "application/json": {
                "schema": schema
            }
        }
    })
}

fn text_response(description: &str) -> Value {
    json!({
        "description": description,
        "content": {
            "text/plain": {
                "schema": {
                    "type": "string"
                }
            }
        }
    })
}

fn error_response(description: &str) -> Value {
    json_response(description, schema_ref("ErrorEnvelope"))
}

fn no_content_response(description: &str) -> Value {
    json!({ "description": description })
}

fn path_uuid_parameter(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "in": "path",
        "required": true,
        "description": description,
        "schema": uuid_schema()
    })
}

fn query_parameter(name: &str, required: bool, description: &str, schema: Value) -> Value {
    json!({
        "name": name,
        "in": "query",
        "required": required,
        "description": description,
        "schema": schema
    })
}

fn limit_query_parameter() -> Value {
    json!({
        "name": "limit",
        "in": "query",
        "required": false,
        "description": "Maximum number of records to return.",
        "schema": {
            "type": "integer",
            "format": "int32",
            "minimum": 1,
            "maximum": 100
        }
    })
}

fn cursor_query_parameter(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "in": "query",
        "required": false,
        "description": description,
        "schema": string_schema()
    })
}

fn idempotency_header_parameter() -> Value {
    json!({
        "name": "Idempotency-Key",
        "in": "header",
        "required": true,
        "description": "Client-supplied idempotency key used to safely replay create requests.",
        "schema": string_schema()
    })
}

fn string_schema() -> Value {
    json!({ "type": "string" })
}

fn uuid_schema() -> Value {
    json!({
        "type": "string",
        "format": "uuid"
    })
}

fn date_time_schema() -> Value {
    json!({
        "type": "string",
        "format": "date-time"
    })
}

fn int32_schema() -> Value {
    json!({
        "type": "integer",
        "format": "int32"
    })
}

fn int64_schema() -> Value {
    json!({
        "type": "integer",
        "format": "int64"
    })
}

fn array_schema(items: Value) -> Value {
    json!({
        "type": "array",
        "items": items
    })
}

fn nullable(mut schema: Value) -> Value {
    if let Value::Object(object) = &mut schema {
        object.insert("nullable".into(), Value::Bool(true));
    }
    schema
}

fn job_payload_schema() -> Value {
    json!({
        "oneOf": [
            schema_ref("TaskExportJobPayload"),
            schema_ref("DueReminderSweepJobPayload"),
            schema_ref("FreeformObject")
        ]
    })
}

fn job_result_payload_schema() -> Value {
    json!({
        "oneOf": [
            schema_ref("TaskExportJobResult"),
            schema_ref("DueReminderSweepJobResult"),
            schema_ref("FreeformObject")
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::document;

    #[test]
    fn document_includes_job_result_endpoint() {
        let document = document();
        assert!(document["paths"]["/v1/jobs/{job_id}/result"].is_object());
    }
}
