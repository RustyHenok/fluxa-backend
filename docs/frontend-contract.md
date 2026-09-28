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

### OAuth login

- `POST /v1/auth/oauth/:provider` with `{ "code", "redirect_uri", "tenant_id"?, "tenant_name"? }`
- supported providers: `google` and `github`; each must be configured server-side with `OAUTH_<PROVIDER>_CLIENT_ID` / `OAUTH_<PROVIDER>_CLIENT_SECRET`, otherwise the endpoint returns `400`
- the client runs the provider's authorization-code flow and posts the resulting `code` (plus the `redirect_uri` used to obtain it); the backend exchanges the code, resolves the identity, and returns the same `AuthResponse` as `POST /v1/auth/login`
- a known provider identity signs into its linked account; an unknown identity is linked to the existing account with the same verified email, or a brand-new account and workspace are provisioned (role `owner`, email pre-verified)
- identities whose provider email is unverified are rejected with `403`; OAuth-provisioned accounts have a random unusable password until the password reset flow is used
- `GET /v1/me/oauth-accounts` (authenticated) lists linked providers as `[{ "provider", "linked_at" }]`
- `DELETE /v1/me/oauth-accounts/:provider` (authenticated) unlinks a provider (`204`; `404` when nothing is linked); the account stays recoverable through the password reset flow on its verified email

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

### Profile (authenticated)

- every `user` payload includes `display_name` (string or null); tenant member listings include each member's `display_name`
- `PATCH /v1/me` with `{ "display_name": "Ada Lovelace" }` → `200` with the updated `user`; 1–100 characters after trimming
- send `{ "display_name": null }` to clear the name; omitted fields stay unchanged

### Notification preferences (authenticated)

- `GET /v1/me/notification-preferences` → `{ "task_due_soon": bool, "task_overdue": bool, "task_commented": bool }`; everything defaults to `true`
- `PATCH /v1/me/notification-preferences` with any subset of those switches → `200` with the full updated object; an empty body is a `400`
- disabled kinds are skipped at enqueue time (due/overdue reminders and comment mails); security and account mails (verification, password reset, invitations) are always sent

### In-app notifications (authenticated)

- `GET /v1/me/notifications` → the active tenant's task-activity feed for the current user, newest first: `{ "data": [{ "id", "kind", "payload", "read_at", "created_at" }], "next_cursor", "unread_count" }`
- kinds are limited to `task_due_soon`, `task_overdue`, and `task_commented`; token-bearing account mails never appear in the feed
- `?unread=true` returns only unread entries; `limit` (max 200) and the opaque `cursor` paginate
- `POST /v1/me/notifications/:notification_id/read` → `204` (idempotent; `404` when the id is not one of the caller's feed entries)
- `POST /v1/me/notifications/read-all` → `{ "updated": n }`

### Sessions (authenticated)

- `GET /v1/me/sessions` → active refresh sessions across all tenants, newest first, as `[{ "id", "tenant_id", "created_at", "expires_at" }]`
- `DELETE /v1/me/sessions/:session_id` → `204` revokes one session (`404` when the id is not one of the caller's active sessions)
- `DELETE /v1/me/sessions` → `204` "log out everywhere": revokes every refresh session and the current access token; access tokens on other devices stay valid until their short TTL expires

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
- `payload_too_large`
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
- `POST /v1/projects`
- `POST /v1/tenants/:tenant_id/invitations`
- `POST /v1/labels`
- `POST /v1/tasks/:task_id/comments`
- `POST /v1/tasks/:task_id/attachments`
- `POST /v1/webhooks`

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
- `PATCH /v1/me`
- `GET /v1/me/notification-preferences`
- `PATCH /v1/me/notification-preferences`
- `GET /v1/me/notifications`
- `POST /v1/me/notifications/read-all`
- `POST /v1/me/notifications/:notification_id/read`
- `GET /v1/me/tenants`
- `GET /v1/me/oauth-accounts`
- `DELETE /v1/me/oauth-accounts/:provider`
- `GET /v1/me/sessions`
- `DELETE /v1/me/sessions`
- `DELETE /v1/me/sessions/:session_id`
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
- `DELETE /v1/projects/:project_id` (soft delete: archives the project)
- `POST /v1/projects/:project_id/restore`
- `GET /v1/projects/:project_id/tasks`
- `GET /v1/tasks`
- `POST /v1/tasks`
- `POST /v1/tasks/bulk/status` (updates up to 100 tasks atomically; returns `{ "updated": n, "data": [...] }`)
- `GET /v1/tasks/:task_id`
- `PATCH /v1/tasks/:task_id` (nullable fields `project_id`, `description`, `assignee_id`, `due_at`: send `null` to clear, omit to leave unchanged)
- `DELETE /v1/tasks/:task_id` (soft delete: sets status to `archived`)
- `POST /v1/tasks/:task_id/restore`
- `GET /v1/tasks/:task_id/audit`
- `GET /v1/labels`
- `POST /v1/labels` (owner/admin)
- `PATCH /v1/labels/:label_id` (owner/admin)
- `DELETE /v1/labels/:label_id` (owner/admin)
- `GET /v1/tasks/:task_id/labels`
- `PUT /v1/tasks/:task_id/labels` (replace the task's label set)
- `GET /v1/tasks/:task_id/comments`
- `POST /v1/tasks/:task_id/comments`
- `PATCH /v1/tasks/:task_id/comments/:comment_id` (author only)
- `DELETE /v1/tasks/:task_id/comments/:comment_id` (author or owner/admin)
- `GET /v1/tasks/:task_id/attachments`
- `POST /v1/tasks/:task_id/attachments` (raw body upload with `?file_name=`)
- `GET /v1/tasks/:task_id/attachments/:attachment_id/download`
- `DELETE /v1/tasks/:task_id/attachments/:attachment_id` (uploader or owner/admin)
- `GET /v1/webhooks` (owner/admin)
- `POST /v1/webhooks` (owner/admin; response includes the signing `secret` once)
- `PATCH /v1/webhooks/:webhook_id` (owner/admin)
- `DELETE /v1/webhooks/:webhook_id` (owner/admin)
- `GET /v1/webhooks/:webhook_id/deliveries` (owner/admin)

### Bulk Status Updates

- `POST /v1/tasks/bulk/status` accepts `{ "task_ids": [...], "status": "done" }` and sets the same status on every task in one transaction
- duplicate ids are ignored; after deduplication the list must contain 1–100 ids, otherwise the API responds `400`
- the update is all-or-nothing: if any id does not belong to a task in the active tenant, nothing is updated and the API responds `404`
- the response is `{ "updated": n, "data": [TaskResponse...] }` with tasks in the same order as the request ids, and each task gains a `task_status_updated` audit entry

### Soft Delete Semantics

- `DELETE /v1/tasks/:task_id` archives the task (status `archived`) instead of removing the row; the task stays readable via `GET /v1/tasks/:task_id` and appears in listings when filtering `status=archived`
- `POST /v1/tasks/:task_id/restore` returns an archived task to `open`; it responds `404` when the task is not archived
- `DELETE /v1/projects/:project_id` archives the project: it disappears from `GET /v1/projects`, direct fetches return `404`, and its tasks are hidden from task listings and exports until the project is restored
- `POST /v1/projects/:project_id/restore` un-archives the project and responds `404` when no archived project matches
- hard purges are handled by backend retention jobs, not by the API

### Task Search

- the `q` filter on task listings uses full-text (web search) matching over title and description for terms of three or more characters — whole words, `"quoted phrases"`, and `-negation` work; matching is on complete words, not substrings
- terms shorter than three characters fall back to case-insensitive substring matching

### Labels

- labels are tenant-scoped; names are unique per tenant (case-insensitive) and `color` is an optional `#rrggbb` hex value
- creating, renaming, recoloring, and deleting labels requires `owner` or `admin`; any member can attach labels to tasks
- `PUT /v1/tasks/:task_id/labels` replaces the task's full label set with `{ "label_ids": [...] }` and returns the resulting labels; sending `[]` clears them
- deleting a label removes it from every task that carried it
- task listings and exports accept a `label_id` filter that returns only tasks carrying that label

### Task Comments

- any member can comment on a task; the body is trimmed and limited to 4000 characters
- `GET /v1/tasks/:task_id/comments` returns newest-first pages with the standard `data` + `next_cursor` envelope (`limit` 1–100, default 20)
- only the comment author can edit a comment; the author or an owner/admin can delete it
- adding or deleting a comment appears in the task audit feed (`task_comment_added` / `task_comment_deleted`)
- when the task has an assignee other than the comment author, the assignee receives a notification

### Task Attachments

- any member can upload attachments by sending the raw file bytes as the request body to `POST /v1/tasks/:task_id/attachments?file_name=...`; the request `Content-Type` header is stored and echoed on download
- uploads require an `Idempotency-Key` header, a non-empty body, and a safe `file_name` (no path separators, quotes, or control characters; at most 255 characters)
- the maximum upload size is `MAX_ATTACHMENT_SIZE_BYTES` (default 5 MiB) — larger requests are rejected with `413`; a task holds at most 20 attachments
- attachment responses include a `download_path` pointing at `GET /v1/tasks/:task_id/attachments/:attachment_id/download`, which streams the bytes with the stored content type
- only the uploader or an owner/admin can delete an attachment; deletion also removes the stored file
- uploads and deletions appear in the task audit feed (`task_attachment_added` / `task_attachment_deleted`)

### Webhooks

- webhook management (all `/v1/webhooks*` endpoints) requires `owner` or `admin`; a tenant may register at most 10 webhooks
- `POST /v1/webhooks` accepts `{ "url": "https://...", "events": [...] }`; supported events are `task_created`, `task_updated`, `task_status_updated`, `task_archived`, and `task_restored`
- the create response is `{ "webhook": {...}, "secret": "..." }` — the signing secret is returned only once and is never included in later reads
- webhook URLs must be `http`/`https`; private-network and loopback hosts are rejected unless the deployment sets `WEBHOOK_ALLOW_PRIVATE_URLS=true`
- deliveries POST a JSON body `{ "id": delivery_id, "event": ..., "created_at": ..., "data": TaskResponse }` with headers `X-Fluxa-Event`, `X-Fluxa-Delivery`, and `X-Fluxa-Signature: sha256=<hex hmac-sha256(secret, raw body)>`
- receivers should verify the signature with a constant-time comparison and respond with a 2xx status; failures are retried with exponential backoff up to 5 attempts, then parked as `dead_letter`
- `GET /v1/webhooks/:webhook_id/deliveries` pages the delivery history (`data` + `next_cursor`, `limit` 1–100, default 20) with per-delivery `status` (`pending` / `delivered` / `dead_letter`), `attempts`, and `last_error`
- `POST /v1/webhooks/:webhook_id/deliveries/:delivery_id/redeliver` requeues a `delivered` or `dead_letter` delivery with a fresh attempt budget and responds `202` with the pending delivery; requeuing an already-`pending` delivery returns `409`
- `PATCH /v1/webhooks/:webhook_id` updates `url`, `events`, and/or `is_active`; disabled webhooks stop receiving new events

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

- generated TypeScript client in `fluxa-web`
- generated Dart client in `fluxa-mobile`
- browser session/BFF implementation in the web repo
- secure token storage implementation in the mobile repo
