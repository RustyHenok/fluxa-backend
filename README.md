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
- `GET /v1/me`
- `GET /v1/me/tenants`
- `GET /v1/tasks`
- `POST /v1/tasks`
- `GET /v1/tasks/:task_id`
- `PATCH /v1/tasks/:task_id`
- `DELETE /v1/tasks/:task_id`
- `POST /v1/exports/tasks`
- `GET /v1/jobs/:job_id`
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

- REST API: `http://127.0.0.1:8080`
- gRPC: `127.0.0.1:50051`
- PostgreSQL: `127.0.0.1:5432`
- Redis: `127.0.0.1:16379` by default

Set `REDIS_HOST_PORT` before `docker compose up --build` if you want a different published Redis port.

The compose file uses a development-only JWT secret and local database credentials. Override them before using the stack outside local development.

## gRPC services

- `fluxa.internal.v1.JobAdmin`
- `fluxa.internal.v1.TaskRead`
