use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use once_cell::sync::Lazy;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Script};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

use crate::config::SharedConfig;
use crate::error::{AppError, AppResult};

static RATE_LIMIT_SCRIPT: Lazy<Script> = Lazy::new(|| {
    Script::new(
        r#"
local key = KEYS[1]
local capacity = tonumber(ARGV[1])
local refill_per_ms = tonumber(ARGV[2])
local now_ms = tonumber(ARGV[3])
local cost = tonumber(ARGV[4])
local ttl_ms = tonumber(ARGV[5])

local data = redis.call("HMGET", key, "tokens", "ts")
local tokens = tonumber(data[1])
local ts = tonumber(data[2])

if not tokens then
  tokens = capacity
  ts = now_ms
end

local elapsed = math.max(0, now_ms - ts)
tokens = math.min(capacity, tokens + elapsed * refill_per_ms)

local allowed = 0
local retry_after_ms = 0
if tokens >= cost then
  tokens = tokens - cost
  allowed = 1
else
  retry_after_ms = math.ceil((cost - tokens) / refill_per_ms)
end

redis.call("HMSET", key, "tokens", tokens, "ts", now_ms)
redis.call("PEXPIRE", key, ttl_ms)

return {allowed, tokens, retry_after_ms}
"#,
    )
});

#[derive(Debug, Clone)]
pub struct CacheStore {
    client: redis::Client,
    config: SharedConfig,
}

#[derive(Debug, Clone)]
pub struct RateLimitDecision {
    pub allowed: bool,
    pub remaining_tokens: u64,
    pub retry_after: Duration,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct StoredResponse {
    pub status: u16,
    pub body: serde_json::Value,
}

#[derive(Debug, Clone)]
pub enum IdempotencyState {
    Empty,
    Pending,
    Ready(StoredResponse),
}

impl CacheStore {
    pub fn new(redis_url: String, config: SharedConfig) -> AppResult<Self> {
        let client = redis::Client::open(redis_url).map_err(|error| {
            AppError::internal(format!("failed to create redis client: {error}"))
        })?;

        Ok(Self { client, config })
    }

    async fn connection(&self) -> AppResult<ConnectionManager> {
        self.client
            .get_connection_manager()
            .await
            .map_err(AppError::from)
    }

    pub async fn ping(&self) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let response: String = redis::cmd("PING")
            .query_async(&mut connection)
            .await
            .map_err(AppError::from)?;
        if response != "PONG" {
            return Err(AppError::internal(format!(
                "unexpected redis ping response: {response}"
            )));
        }

        Ok(())
    }

    pub async fn get_json<T: DeserializeOwned>(&self, key: &str) -> AppResult<Option<T>> {
        let mut connection = self.connection().await?;
        let payload: Option<String> = connection.get(key).await?;
        match payload {
            Some(payload) => Ok(Some(serde_json::from_str(&payload).map_err(|error| {
                AppError::internal(format!("failed to deserialize cached payload: {error}"))
            })?)),
            None => Ok(None),
        }
    }

    pub async fn set_json<T: Serialize>(
        &self,
        key: &str,
        value: &T,
        ttl: Duration,
    ) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let payload = serde_json::to_string(value).map_err(|error| {
            AppError::internal(format!("failed to serialize cached payload: {error}"))
        })?;
        let _: () = connection
            .set_ex(key, payload, ttl.as_secs())
            .await
            .map_err(AppError::from)?;
        Ok(())
    }

    pub async fn tenant_cache_version(&self, tenant_id: Uuid) -> AppResult<u64> {
        let mut connection = self.connection().await?;
        let key = format!("cache:tenant:{tenant_id}:version");
        let version: Option<u64> = connection.get(&key).await?;
        Ok(version.unwrap_or(0))
    }

    pub async fn bump_tenant_cache_version(&self, tenant_id: Uuid) -> AppResult<u64> {
        let mut connection = self.connection().await?;
        let key = format!("cache:tenant:{tenant_id}:version");
        connection.incr(&key, 1).await.map_err(AppError::from)
    }

    pub async fn enqueue_job(&self, job_id: Uuid) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let _: usize = connection
            .rpush(&self.config.job_queue_name, job_id.to_string())
            .await?;
        Ok(())
    }

    pub async fn dequeue_job(&self, timeout_seconds: usize) -> AppResult<Option<Uuid>> {
        let mut connection = self.connection().await?;
        let result: Option<[String; 2]> = connection
            .blpop(&self.config.job_queue_name, timeout_seconds as f64)
            .await?;

        result
            .map(|[_queue, value]| {
                Uuid::parse_str(&value)
                    .map_err(|error| AppError::internal(format!("invalid queued job id: {error}")))
            })
            .transpose()
    }

    pub async fn rate_limit(
        &self,
        key: &str,
        capacity: u64,
        refill_per_sec: u64,
        cost: u64,
    ) -> AppResult<RateLimitDecision> {
        let now_ms = now_millis();
        let capacity_milli = capacity * 1000;
        let cost_milli = cost * 1000;
        let refill_per_ms = refill_per_sec;
        let ttl_ms = ((capacity_milli / refill_per_ms) + 1_000).max(1_000);
        let mut connection = self.connection().await?;

        let response: (i64, i64, i64) = RATE_LIMIT_SCRIPT
            .key(key)
            .arg(capacity_milli as i64)
            .arg(refill_per_ms as i64)
            .arg(now_ms as i64)
            .arg(cost_milli as i64)
            .arg(ttl_ms as i64)
            .invoke_async(&mut connection)
            .await?;

        Ok(RateLimitDecision {
            allowed: response.0 == 1,
            remaining_tokens: (response.1.max(0) as u64) / 1000,
            retry_after: Duration::from_millis(response.2.max(0) as u64),
        })
    }

    pub async fn idempotency_state(&self, key: &str) -> AppResult<IdempotencyState> {
        let mut connection = self.connection().await?;
        let payload: Option<String> = connection.get(key).await?;
        match payload {
            None => Ok(IdempotencyState::Empty),
            Some(payload) if payload == "__pending__" => Ok(IdempotencyState::Pending),
            Some(payload) => Ok(IdempotencyState::Ready(
                serde_json::from_str(&payload).map_err(|error| {
                    AppError::internal(format!(
                        "failed to deserialize idempotency payload: {error}"
                    ))
                })?,
            )),
        }
    }

    pub async fn claim_idempotency_key(&self, key: &str, ttl: Duration) -> AppResult<bool> {
        let mut connection = self.connection().await?;
        let claimed: bool = connection.set_nx(key, "__pending__").await?;
        if claimed {
            let _: bool = connection.expire(key, ttl.as_secs() as i64).await?;
        }
        Ok(claimed)
    }

    pub async fn store_idempotency_response(
        &self,
        key: &str,
        response: &StoredResponse,
        ttl: Duration,
    ) -> AppResult<()> {
        self.set_json(key, response, ttl).await
    }

    pub async fn delete_key(&self, key: &str) -> AppResult<()> {
        let mut connection = self.connection().await?;
        let _: usize = connection.del(key).await?;
        Ok(())
    }

    pub fn task_list_cache_key<T: Serialize>(
        &self,
        tenant_id: Uuid,
        version: u64,
        payload: &T,
    ) -> AppResult<String> {
        cache_key("cache:tasks:list", tenant_id, version, payload)
    }

    pub fn task_detail_cache_key(&self, tenant_id: Uuid, version: u64, task_id: Uuid) -> String {
        format!("cache:tasks:detail:{tenant_id}:{version}:{task_id}")
    }

    pub fn idempotency_key(&self, tenant_id: Uuid, route: &str, key: &str) -> String {
        let encoded = URL_SAFE_NO_PAD.encode(key.as_bytes());
        format!("idempotency:{tenant_id}:{route}:{encoded}")
    }
}

fn cache_key<T: Serialize>(
    prefix: &str,
    tenant_id: Uuid,
    version: u64,
    payload: &T,
) -> AppResult<String> {
    let payload = serde_json::to_vec(payload).map_err(|error| {
        AppError::internal(format!("failed to serialize cache key payload: {error}"))
    })?;
    let encoded = URL_SAFE_NO_PAD.encode(payload);
    Ok(format!("{prefix}:{tenant_id}:{version}:{encoded}"))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::now_millis;

    #[test]
    fn time_moves_forward() {
        let first = now_millis();
        let second = now_millis();
        assert!(second >= first);
    }
}
