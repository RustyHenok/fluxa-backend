# Frontend Contract Guide

This document is the handoff point for `fluxa-web` and `fluxa-mobile`.

## Source Of Truth

- Machine-readable contract: `openapi/fluxa-openapi.json`
- Generator: `scripts/generate_openapi.sh`
- CI check: `.github/workflows/ci.yml`

If the public REST API changes, regenerate the OpenAPI file and keep this guide aligned with the behavior change.

## Client Split

- `fluxa-web`
  - stack target: `Next.js + TypeScript`
  - auth model: prefer BFF or server-side cookie handling for refresh tokens
- `fluxa-mobile`
  - stack target: `Flutter`
  - auth model: keep refresh token in secure storage and access token in memory where practical

Both clients should use the REST API. The gRPC surface stays internal-only.

## Auth Flow

### Register

- `POST /v1/auth/register`
- returns:
  - `access_token`
  - `refresh_token`
  - `expires_in_seconds`
  - `user`
  - `active_tenant`

### Login

- `POST /v1/auth/login`
- optional `tenant_id` lets the client log directly into a selected tenant membership

### Refresh

- `POST /v1/auth/refresh`
- refresh token rotation is enabled
- the previous refresh token becomes invalid after a successful refresh

### Logout

- `POST /v1/auth/logout`
- revokes the supplied refresh token

### Switch Tenant

- `POST /v1/auth/switch-tenant`
- authenticated endpoint
- request body:

```json
{
  "tenant_id": "uuid"
}
```

- returns the same shape as login/refresh with a newly scoped session

## Error Envelope

All REST failures use the same envelope:

```json
{
  "error": {
    "code": "string_code",
    "message": "human readable message"
  }
}
```

Common codes:

- `validation_error`
- `unauthorized`
- `forbidden`
- `not_found`
- `conflict`
- `rate_limited`
- `internal_error`

## Pagination

Cursor pagination is used for task lists and task audit feeds.

- request params:
  - `limit`
  - `cursor`
- response fields:
  - `data`
  - `next_cursor`

Rules:

- cursors are opaque
- `next_cursor = null` means the client reached the end
- current page size is bounded to `1..100`

## Idempotency

The following create endpoints require `Idempotency-Key`:

- `POST /v1/tasks`
- `POST /v1/exports/tasks`

Client expectation:

- retrying with the same payload and same key should replay the original response
- retrying while the original request is still in progress may return `409 conflict`

## Key Endpoints For Web And Mobile

### Session And Tenancy

- `POST /v1/auth/register`
- `POST /v1/auth/login`
- `POST /v1/auth/refresh`
- `POST /v1/auth/logout`
- `POST /v1/auth/switch-tenant`
- `GET /v1/me`
- `GET /v1/me/tenants`
- `GET /v1/tenants/:tenant_id/members`

### Tasks

- `GET /v1/dashboard/summary`
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/:task_id`
- `PATCH /v1/tasks/:task_id`
- `DELETE /v1/tasks/:task_id`
- `GET /v1/tasks/:task_id/audit`

### Jobs / Exports

- `POST /v1/exports/tasks`
- `GET /v1/jobs/:job_id`
- `GET /v1/jobs/:job_id/result`

Recommended client behavior:

- poll `GET /v1/jobs/:job_id` until status is `completed`
- then fetch `GET /v1/jobs/:job_id/result`
- do not rely on `result_payload` embedded inside the status response as the primary frontend contract

## Stable Enum Values

### Membership roles

- `owner`
- `admin`
- `member`

### Task status

- `open`
- `in_progress`
- `done`
- `archived`

### Task priority

- `low`
- `medium`
- `high`
- `urgent`

### Job status

- `queued`
- `running`
- `completed`
- `dead_letter`

### Job type

- `task_export`
- `due_reminder_sweep`

## Recommended Frontend Setup

Each frontend repo should keep a synced copy of the OpenAPI contract from `fluxa-backend`.

Expected local mono-workspace shape:

```text
fluxa/
  fluxa-backend/
  fluxa-web/
  fluxa-mobile/
```

Use the repo-local sync scripts in:

- `fluxa-web/scripts/sync_openapi.sh`
- `fluxa-mobile/scripts/sync_openapi.sh`

Those scripts copy the checked-in backend contract into each frontend repo under `contracts/fluxa-openapi.json`.

## Current Gaps

The contract is ready for client work, but these are still follow-up improvements rather than blockers:

- generated TypeScript client in `fluxa-web`
- generated Dart client in `fluxa-mobile`
- browser session/BFF implementation in the web repo
- secure token storage implementation in the mobile repo
