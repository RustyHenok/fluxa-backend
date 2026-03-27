# fluxa-backend

Enterprise-grade multi-tenant task platform built with `axum`, `tokio`, `sqlx`, `redis`, and `tonic`.

## Features

- Public REST API for auth, tenant-aware task management, export jobs, health checks, and metrics
- Internal gRPC API for job administration and task read access
- PostgreSQL-backed system of record with committed SQLx migrations
- Redis-backed caching, idempotency handling, rate limiting, and job queue coordination
- JWT access and refresh tokens with rotation
- Background workers for task exports and due reminder sweeps

## Main endpoints

- `POST /v1/auth/register`
- `POST /v1/auth/login`
- `POST /v1/auth/refresh`
- `POST /v1/auth/logout`
- `POST /v1/auth/switch-tenant`
- `GET /v1/dashboard/summary`
- `GET /v1/me`
- `GET /v1/me/tenants`
- `GET /v1/tenants/:tenant_id/members`
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/:task_id`
- `GET /v1/tasks/:task_id/audit`
- `PATCH /v1/tasks/:task_id`
- `DELETE /v1/tasks/:task_id`
- `POST /v1/exports/tasks`
- `GET /v1/jobs/:job_id`
- `GET /v1/jobs/:job_id/result`
- `GET /healthz`
- `GET /readyz`
- `GET /metrics`

## Run locally

1. Copy `.env.example` into `.env` and adjust the values for PostgreSQL, Redis, and `JWT_SECRET`.
2. Start PostgreSQL and Redis locally.
3. Run the service:

```bash
cargo run -- --mode all
```

Use `--mode api` to run only the HTTP and gRPC servers, or `--mode worker` to run just the background worker.

## Run with Docker Compose

Bring up PostgreSQL, Redis, the API, and the worker as separate services:

```bash
docker compose up --build
```

Important local ports:

- REST API: `http://127.0.0.1:18080` by default
- gRPC: `127.0.0.1:15051` by default
- PostgreSQL: `127.0.0.1:5432`
- Redis: `127.0.0.1:16379` by default

Set `HTTP_HOST_PORT`, `GRPC_HOST_PORT`, or `REDIS_HOST_PORT` before `docker compose up --build` if you want different published ports.

The compose file uses a development-only JWT secret and local database credentials. Override them before using the stack outside local development.

## Smoke test

Run the end-to-end smoke test after the stack is up:

```bash
./scripts/smoke_test.sh
```

The script checks health and readiness, registers a tenant owner, exercises authenticated task CRUD paths, verifies task create idempotency, waits for an export job to complete, fetches the dedicated job result endpoint, and validates refresh plus logout. Set `BASE=http://127.0.0.1:18080` explicitly if you changed the published API port.

It also waits for the API to become ready, checks the standard error envelope on unauthorized requests, and confirms the Prometheus metrics endpoint is emitting request counters.

## OpenAPI contract

Generate the checked-in OpenAPI document with:

```bash
./scripts/generate_openapi.sh
```

The generated contract is written to `openapi/fluxa-openapi.json`, and CI verifies that the committed file stays in sync with the generator.

For the deeper Docker-backed integration suite, run:

```bash
TEST_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5432/fluxa \
TEST_REDIS_URL=redis://127.0.0.1:16379/ \
cargo test --test stack_contracts -- --ignored --nocapture
```

## gRPC services

- `fluxa.internal.v1.JobAdmin`
- `fluxa.internal.v1.TaskRead`
