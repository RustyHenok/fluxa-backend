#!/usr/bin/env bash
set -euo pipefail

BASE="${BASE:-http://127.0.0.1:18080}"
PASSWORD="${PASSWORD:-supersecret123}"
TENANT_NAME="${TENANT_NAME:-Smoke Tenant}"
EMAIL="smoke-$(date +%s)-$RANDOM@example.com"

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'missing required command: %s\n' "$1" >&2
    exit 1
  fi
}

step() {
  printf '\n==> %s\n' "$1"
}

fail() {
  printf 'smoke test failed: %s\n' "$1" >&2
  exit 1
}

require_cmd curl
require_cmd jq

step "Health checks"
HEALTH_JSON="$(curl -sS "$BASE/healthz")"
READY_JSON="$(curl -sS "$BASE/readyz")"

[[ "$(jq -r '.status' <<<"$HEALTH_JSON")" == "ok" ]] || fail "/healthz did not return ok"
[[ "$(jq -r '.status' <<<"$READY_JSON")" == "ready" ]] || fail "/readyz did not return ready"

step "Register tenant owner"
AUTH_JSON="$(
  curl -sS -X POST "$BASE/v1/auth/register" \
    -H 'content-type: application/json' \
    -d "{\"email\":\"$EMAIL\",\"password\":\"$PASSWORD\",\"tenant_name\":\"$TENANT_NAME\"}"
)"

ACCESS_TOKEN="$(jq -er '.access_token' <<<"$AUTH_JSON")"
REFRESH_TOKEN="$(jq -er '.refresh_token' <<<"$AUTH_JSON")"
TENANT_ID="$(jq -er '.active_tenant.tenant_id' <<<"$AUTH_JSON")"
USER_ID="$(jq -er '.user.id' <<<"$AUTH_JSON")"

step "Verify authenticated reads"
ME_JSON="$(curl -sS "$BASE/v1/me" -H "Authorization: Bearer $ACCESS_TOKEN")"
TENANTS_JSON="$(curl -sS "$BASE/v1/me/tenants" -H "Authorization: Bearer $ACCESS_TOKEN")"

[[ "$(jq -r '.user.id' <<<"$ME_JSON")" == "$USER_ID" ]] || fail "/v1/me returned unexpected user"
[[ "$(jq -r '.[0].tenant_id' <<<"$TENANTS_JSON")" == "$TENANT_ID" ]] || fail "/v1/me/tenants returned unexpected tenant"

step "Create task with idempotency replay"
TASK_KEY="task-create-$(date +%s)-$RANDOM"
TASK_PAYLOAD='{"title":"Smoke task","description":"verify patch and idempotency","status":"open","priority":"high"}'

TASK_JSON="$(
  curl -sS -X POST "$BASE/v1/tasks" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H 'content-type: application/json' \
    -H "Idempotency-Key: $TASK_KEY" \
    -d "$TASK_PAYLOAD"
)"
TASK_ID="$(jq -er '.id' <<<"$TASK_JSON")"

TASK_REPLAY_JSON="$(
  curl -sS -X POST "$BASE/v1/tasks" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H 'content-type: application/json' \
    -H "Idempotency-Key: $TASK_KEY" \
    -d "$TASK_PAYLOAD"
)"

[[ "$(jq -r '.id' <<<"$TASK_REPLAY_JSON")" == "$TASK_ID" ]] || fail "task idempotency replay returned a different task id"

step "List and patch task"
LIST_JSON="$(curl -sS "$BASE/v1/tasks?limit=10&status=open&priority=high" -H "Authorization: Bearer $ACCESS_TOKEN")"
[[ "$(jq -r '.data[0].id' <<<"$LIST_JSON")" == "$TASK_ID" ]] || fail "task list did not include created task"

PATCH_JSON="$(
  curl -sS -X PATCH "$BASE/v1/tasks/$TASK_ID" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H 'content-type: application/json' \
    -d '{"status":"in_progress","priority":"urgent"}'
)"

[[ "$(jq -r '.status' <<<"$PATCH_JSON")" == "in_progress" ]] || fail "task patch did not update status"
[[ "$(jq -r '.priority' <<<"$PATCH_JSON")" == "urgent" ]] || fail "task patch did not update priority"

step "Create export job and wait for completion"
EXPORT_KEY="task-export-$(date +%s)-$RANDOM"
JOB_JSON="$(
  curl -sS -X POST "$BASE/v1/exports/tasks" \
    -H "Authorization: Bearer $ACCESS_TOKEN" \
    -H 'content-type: application/json' \
    -H "Idempotency-Key: $EXPORT_KEY" \
    -d '{"status":"in_progress"}'
)"
JOB_ID="$(jq -er '.id' <<<"$JOB_JSON")"

JOB_STATUS=""
for _ in $(seq 1 10); do
  JOB_DETAILS="$(curl -sS "$BASE/v1/jobs/$JOB_ID" -H "Authorization: Bearer $ACCESS_TOKEN")"
  JOB_STATUS="$(jq -r '.status' <<<"$JOB_DETAILS")"
  if [[ "$JOB_STATUS" == "completed" ]]; then
    break
  fi
  sleep 1
done

[[ "$JOB_STATUS" == "completed" ]] || fail "export job did not complete in time"

step "Refresh and logout"
REFRESH_JSON="$(
  curl -sS -X POST "$BASE/v1/auth/refresh" \
    -H 'content-type: application/json' \
    -d "{\"refresh_token\":\"$REFRESH_TOKEN\"}"
)"
NEXT_REFRESH_TOKEN="$(jq -er '.refresh_token' <<<"$REFRESH_JSON")"

LOGOUT_STATUS="$(
  curl -sS -o /dev/null -w '%{http_code}' -X POST "$BASE/v1/auth/logout" \
    -H 'content-type: application/json' \
    -d "{\"refresh_token\":\"$NEXT_REFRESH_TOKEN\"}"
)"
[[ "$LOGOUT_STATUS" == "204" ]] || fail "logout did not return 204"

printf '\nSmoke test passed for %s\n' "$BASE"
printf 'tenant_id=%s\n' "$TENANT_ID"
printf 'task_id=%s\n' "$TASK_ID"
printf 'job_id=%s\n' "$JOB_ID"
