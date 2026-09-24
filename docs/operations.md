# Operations Runbook

Operational guidance for running fluxa-backend outside local development.

## Processes and scaling

The binary runs in one of three modes (`APP_MODE`):

- `api` — HTTP (REST) and gRPC servers
- `worker` — background loops: job dispatch, job processing, due-reminder + retention scheduling, stale-job reaping, notification delivery
- `all` — everything in one process (local development)

Scaling notes:

- **API replicas** are stateless; scale horizontally at will.
- **Worker replicas** are safe to scale horizontally: job claims use an atomic `mark_job_running` update, the stale-job reaper re-queues expired leases, and notification claims use `FOR UPDATE SKIP LOCKED`.
- Every process runs its own metrics recorder; scrape `/metrics` per API pod. Worker metrics are process-local; run workers with the API in `all` mode only when a single process is acceptable.

## Required configuration

Secrets (set via your secret manager, never in images):

| Variable | Notes |
| --- | --- |
| `DATABASE_URL` | PostgreSQL connection string |
| `REDIS_URL` | Redis connection string |
| `JWT_SECRET` | ≥ 32 chars, unique per environment |
| `GRPC_AUTH_TOKEN` | ≥ 32 chars, shared with internal gRPC callers only |
| `SMTP_URL` / `MAIL_FROM` | only when `MAILER_PROVIDER=smtp` |
| `METRICS_AUTH_TOKEN` | optional; when set, `/metrics` requires this bearer token |

The service warns at startup when `CORS_ALLOW_ORIGIN=*` or when the docker-compose development `JWT_SECRET`/`GRPC_AUTH_TOKEN` values are detected — treat those warnings as deploy blockers outside local development.

## Probes

- `GET /healthz` — liveness: process is up.
- `GET /readyz` — readiness: verifies PostgreSQL and Redis connectivity. Wire this to readiness probes so pods are only routed traffic once migrations have applied and dependencies are reachable (migrations run automatically at startup).

## Metrics

`GET /metrics` serves Prometheus text format (optionally guarded by `METRICS_AUTH_TOKEN`). Key series:

- `http_requests_total{method,route,status}` and `http_request_duration_seconds{method,route}` — per matched route
- `db_pool_connections{state}` — open/idle pool connections (sampled every `SAMPLER_INTERVAL_MS`)
- `jobs_queued` (database) and `job_queue_depth` (Redis dispatch list)
- `jobs_completed_total{job_type}`, `jobs_failed_total{job_type}`, `jobs_reaped_total`
- `notifications_sent_total`, `notifications_failed_total`
- `retention_rows_purged_total{table}`

Distributed tracing: structured JSON logs include `x-request-id` propagation today. OTLP trace export is a planned follow-up; until then, forward logs to your aggregator and correlate on request id.

## Data retention

A `retention_sweep` background job runs on a cadence (`RETENTION_SWEEP_INTERVAL_HOURS`, default 24) through the normal job machinery and deletes:

| Data | Window | Variable |
| --- | --- | --- |
| Expired/revoked refresh tokens | 30 days | `REFRESH_TOKEN_RETENTION_DAYS` |
| Completed/dead-letter background jobs | 30 days | `JOB_RETENTION_DAYS` |
| Sent/dead-letter notifications | 30 days | `NOTIFICATION_RETENTION_DAYS` |
| Audit log rows | 365 days | `AUDIT_RETENTION_DAYS` |

Per-run deletion counts land in the job's `result_payload` and in `retention_rows_purged_total`.

## Backup and restore

- PostgreSQL is the system of record — schedule `pg_dump` (or provider snapshots) and test restores; migrations are additive and re-applied automatically on startup.
- Redis holds only rebuildable state (cache, rate limits, idempotency replays, dispatch queue). After a Redis loss, queued-but-undispatched jobs are re-enqueued by the worker dispatch loop from the database; no backup is required.
- Export artifacts live under `ARTIFACT_STORAGE_DIR`; back the volume up if download continuity matters, or treat exports as re-runnable.

## Deployment

Reference Kubernetes manifests live in [`deploy/kubernetes/`](../deploy/kubernetes/). They model:

- separate `api` and `worker` Deployments from the same image
- readiness/liveness probes on `/readyz` and `/healthz`
- resource requests/limits and secret-backed environment
- a shared PVC for export artifacts (swap for object storage when available)

The gRPC port carries internal admin APIs: keep it cluster-internal (no Ingress), require the shared token, and add mTLS between peers as the next hardening step.
