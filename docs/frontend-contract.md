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
- optionally revokes the paired access token for the rest of its lifetime: send it as `access_token` in the body, or include it as the `Authorization` bearer header
- request body:

```json
{
  "refresh_token": "opaque-token",
  "access_token": "jwt (optional)"
}
```

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

## Account Lifecycle

### Email verification

- registration automatically enqueues a verification email (delivered per the backend's mailer configuration; the `noop` default sends nothing)
- `POST /v1/auth/verify-email` with `{ "token": "..." }` → `204`; the token is single-use and expires (default 24h)
- `POST /v1/auth/resend-verification` with `{ "email": "..." }` → always `202` (no account enumeration)
- `user.email_verified` (boolean) is included in every `user` payload
- when the backend sets `REQUIRE_EMAIL_VERIFICATION=true`, login and refresh return `403` with code `forbidden` until the email is verified; the flag defaults to off

### Password reset

- `POST /v1/auth/password-reset/request` with `{ "email": "..." }` → always `202`
- `POST /v1/auth/password-reset/confirm` with `{ "token": "...", "new_password": "..." }` → `204`; single-use expiring token (default 60 minutes); revokes every refresh token the user holds, so all sessions must log in again

### Credential changes (authenticated)

- `POST /v1/me/change-password` with `{ "current_password": "...", "new_password": "..." }` → `204`; revokes all refresh tokens — clients should treat this as a global logout and re-authenticate
- `POST /v1/me/change-email` with `{ "current_password": "...", "new_email": "..." }` → `200` with the updated `user`; the new address starts unverified and receives a verification email

### Password policy

- 10–128 characters; a small list of very common passwords is rejected with `400 validation_error`

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
- `POST /v1/auth/verify-email`
- `POST /v1/auth/resend-verification`
- `POST /v1/auth/password-reset/request`
- `POST /v1/auth/password-reset/confirm`
- `POST /v1/auth/switch-tenant`
- `GET /v1/me`
- `GET /v1/me/tenants`
- `POST /v1/me/change-password`
- `POST /v1/me/change-email`
- `GET /v1/tenants/:tenant_id/members`
- `PATCH /v1/tenants/:tenant_id/members/:member_id` (change a member's role)
- `DELETE /v1/tenants/:tenant_id/members/:member_id` (remove a member)
- `GET /v1/tenants/:tenant_id/invitations` (owner/admin)
- `POST /v1/tenants/:tenant_id/invitations` (owner/admin)
- `POST /v1/tenants/:tenant_id/invitations/accept`
- `DELETE /v1/tenants/:tenant_id/invitations/:invitation_id` (owner/admin)

### Member Management Rules

- inviting an `admin`, or granting/revoking `owner`/`admin` roles, requires the `owner` role; other member management requires `owner` or `admin`
- invited roles are limited to `admin` and `member`
- a tenant always retains at least one `owner`; demoting or removing the last owner returns `409`
- `POST .../invitations` returns the single-use invitation `token` exactly once in the response so the inviter can share it out of band; an invitation email carrying the same token is also enqueued through the notifications outbox when the backend's mailer is configured (`noop` default sends nothing)
- the invitee calls `POST /v1/tenants/:tenant_id/invitations/accept` with `{ "token": "..." }` while authenticated; the invitation must match their account email and is single-use with an expiry (default 72h)
- removing a member revokes that member's refresh tokens for the tenant

### Tasks

- `GET /v1/dashboard/summary`
- `GET /v1/projects`
- `POST /v1/projects`
- `GET /v1/projects/:project_id`
- `GET /v1/projects/:project_id/summary`
- `PATCH /v1/projects/:project_id`
- `DELETE /v1/projects/:project_id`
- `GET /v1/projects/:project_id/tasks`
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/:task_id`
- `PATCH /v1/tasks/:task_id`
- `DELETE /v1/tasks/:task_id`
- `GET /v1/tasks/:task_id/audit`

### Jobs / Exports

- `POST /v1/exports/tasks` — optional `format` field: `json` (default) or `csv`
- `GET /v1/jobs/:job_id`
- `GET /v1/jobs/:job_id/result`
- `GET /v1/jobs/:job_id/artifact`

Recommended client behavior:

- poll `GET /v1/jobs/:job_id` until status is `completed`
- then fetch `GET /v1/jobs/:job_id/result`; the export result contains `task_count`, `format`, and an `artifact` object (`key`, `content_type`, `size_bytes`, `download_path`) instead of inline task rows
- download the file from `GET /v1/jobs/:job_id/artifact` (authenticated; responds with a `Content-Disposition` attachment)
- do not rely on `result_payload` embedded inside the status response as the primary frontend contract

### Audit Trail (admin/owner only)

- `GET /v1/audit?limit=&cursor=` — tenant-scoped audit events, newest first, keyset pagination via `next_cursor`
- each event: `id`, `actor_user_id`, `subject_type`, `subject_id`, `event_type`, `payload`, `created_at`
- covered events include `auth.login_succeeded`, `auth.login_failed`, `auth.token_refreshed`, `auth.logged_out`, `user.registered`, `account.*`, `project.*`, `invitation.*`, and `member.*`

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

## Project Hierarchy

- tasks may now include an optional `project_id`
- `GET /v1/tasks` supports `project_id` filtering
- `GET /v1/projects/:project_id/summary` returns project-level task counters and recent activity counts
- `GET /v1/projects/:project_id/tasks` provides project-scoped task listing with the same cursor/filter behavior
- `POST /v1/tasks` and `PATCH /v1/tasks/:task_id` may include `project_id`
- project access stays tenant-scoped, just like task access

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

- OAuth/social login is deferred; the `oauth_accounts` table exists but no endpoints are exposed yet
- generated TypeScript client in `fluxa-web`
- generated Dart client in `fluxa-mobile`
- browser session/BFF implementation in the web repo
- secure token storage implementation in the mobile repo
