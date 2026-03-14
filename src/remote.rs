use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use redis::AsyncCommands;
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex as AsyncMutex;

use crate::backend::{InvalidationSubscriber, RemoteBackend, StoredEntry, StoredValue};
use crate::error::{CacheError, CacheResult};

// Redis GET/MGET responses do not include per-key TTL metadata.
// This placeholder keeps `StoredEntry` non-expired for value transport only.
// Real expiration remains enforced by Redis through PSETEX at write time.
const REDIS_READ_PLACEHOLDER_TTL: Duration = Duration::from_secs(3600);

/// JSON wire format persisted in Redis value bytes.
#[derive(Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
enum RedisStoredValue<V> {
    /// Real business value.
    Value(V),
    /// Negative-cache marker.
    Null,
}

/// Remote backend implemented with Redis.
#[derive(Clone)]
pub struct RedisBackend<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Redis client handle.
    client: redis::Client,
    /// Optional key prefix to isolate logical business domains.
    key_prefix: String,
    /// Lazily initialized shared multiplexed connection.
    connection: Arc<AsyncMutex<Option<redis::aio::MultiplexedConnection>>>,
    /// Type marker.
    _marker: PhantomData<V>,
}

impl<V> RedisBackend<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Creates a new redis backend.
    fn new(client: redis::Client, key_prefix: String) -> Self {
        Self {
            client,
            key_prefix,
            connection: Arc::new(AsyncMutex::new(None)),
            _marker: PhantomData,
        }
    }

    /// Applies configured key prefix.
    fn prefixed_key(&self, key: &str) -> String {
        if self.key_prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}:{}", self.key_prefix, key)
        }
    }

    /// Encodes stored value into json bytes.
    fn to_payload(value: StoredValue<V>) -> CacheResult<Vec<u8>> {
        let payload = match value {
            StoredValue::Value(value) => RedisStoredValue::Value(value),
            StoredValue::Null => RedisStoredValue::Null,
        };

        serde_json::to_vec(&payload)
            .map_err(|err| CacheError::Backend(format!("serialize redis payload failed: {err}")))
    }

    /// Decodes json bytes into stored value.
    fn from_payload(bytes: &[u8]) -> CacheResult<StoredValue<V>> {
        let value: RedisStoredValue<V> = serde_json::from_slice(bytes).map_err(|err| {
            CacheError::Backend(format!("deserialize redis payload failed: {err}"))
        })?;

        Ok(match value {
            RedisStoredValue::Value(value) => StoredValue::Value(value),
            RedisStoredValue::Null => StoredValue::Null,
        })
    }

    /// Converts absolute expiration time to redis millisecond TTL.
    fn ttl_ms(expire_at: Instant) -> u64 {
        expire_at
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as u64
    }

    /// Opens a multiplexed async redis connection.
    async fn connection(&self) -> CacheResult<redis::aio::MultiplexedConnection> {
        let mut slot = self.connection.lock().await;
        if let Some(conn) = slot.as_ref() {
            return Ok(conn.clone());
        }

        let conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| CacheError::Backend(format!("redis connection failed: {err}")))?;
        *slot = Some(conn.clone());
        Ok(conn)
    }

    /// Reads one key from redis.
    pub async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        let full_key = self.prefixed_key(key);
        let mut conn = self.connection().await?;
        let raw: Option<Vec<u8>> = conn
            .get(&full_key)
            .await
            .map_err(|err| CacheError::Backend(format!("redis GET failed: {err}")))?;

        let Some(raw) = raw else {
            return Ok(None);
        };

        let value = Self::from_payload(&raw)?;
        Ok(Some(StoredEntry {
            value,
            expire_at: Instant::now() + REDIS_READ_PLACEHOLDER_TTL,
        }))
    }

    /// Reads multiple keys with Redis MGET and returns key-indexed map.
    pub async fn mget(
        &self,
        keys: &[String],
    ) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>> {
        let full_keys = keys
            .iter()
            .map(|key| self.prefixed_key(key))
            .collect::<Vec<_>>();

        let mut conn = self.connection().await?;
        let raw_values: Vec<Option<Vec<u8>>> = conn
            .mget(&full_keys)
            .await
            .map_err(|err| CacheError::Backend(format!("redis MGET failed: {err}")))?;

        let mut values = HashMap::with_capacity(keys.len());
        for (idx, key) in keys.iter().enumerate() {
            let entry = match raw_values.get(idx).cloned().flatten() {
                Some(raw) => Some(StoredEntry {
                    value: Self::from_payload(&raw)?,
                    expire_at: Instant::now() + REDIS_READ_PLACEHOLDER_TTL,
                }),
                None => None,
            };
            values.insert(key.clone(), entry);
        }

        Ok(values)
    }

    /// Writes one key using Redis PSETEX.
    pub async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()> {
        let full_key = self.prefixed_key(key);
        let payload = Self::to_payload(entry.value)?;
        let ttl_ms = Self::ttl_ms(entry.expire_at);

        let mut conn = self.connection().await?;
        let _: () = conn
            .pset_ex(&full_key, payload, ttl_ms)
            .await
            .map_err(|err| CacheError::Backend(format!("redis PSETEX failed: {err}")))?;
        Ok(())
    }

    /// Writes multiple keys with pipeline PSETEX commands.
    pub async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()> {
        let mut pipe = redis::pipe();
        for (key, entry) in entries {
            let payload = Self::to_payload(entry.value)?;
            let ttl_ms = Self::ttl_ms(entry.expire_at);
            let full_key = self.prefixed_key(&key);

            pipe.pset_ex(full_key, payload, ttl_ms).ignore();
        }

        let mut conn = self.connection().await?;
        let _: () = pipe
            .query_async(&mut conn)
            .await
            .map_err(|err| CacheError::Backend(format!("redis pipeline mset failed: {err}")))?;
        Ok(())
    }

    /// Publishes invalidation payload to a Redis channel.
    pub async fn publish(&self, channel: &str, payload: &str) -> CacheResult<()> {
        let mut conn = self.connection().await?;
        let _: usize = conn
            .publish(channel, payload)
            .await
            .map_err(|err| CacheError::Backend(format!("redis PUBLISH failed: {err}")))?;
        Ok(())
    }

    /// Subscribes to a Redis Pub/Sub channel.
    pub async fn subscribe(&self, channel: &str) -> CacheResult<redis::aio::PubSub> {
        let mut pubsub =
            self.client.get_async_pubsub().await.map_err(|err| {
                CacheError::Backend(format!("redis pubsub connect failed: {err}"))
            })?;
        pubsub
            .subscribe(channel)
            .await
            .map_err(|err| CacheError::Backend(format!("redis SUBSCRIBE failed: {err}")))?;
        Ok(pubsub)
    }

    /// Deletes one key from Redis.
    pub async fn del(&self, key: &str) -> CacheResult<()> {
        let full_key = self.prefixed_key(key);
        let mut conn = self.connection().await?;
        let _: usize = conn
            .del(&full_key)
            .await
            .map_err(|err| CacheError::Backend(format!("redis DEL failed: {err}")))?;
        Ok(())
    }

    /// Deletes multiple keys from Redis.
    pub async fn mdel(&self, keys: &[String]) -> CacheResult<()> {
        let full_keys: Vec<String> = keys.iter().map(|key| self.prefixed_key(key)).collect();
        let mut conn = self.connection().await?;
        let _: usize = conn
            .del(full_keys)
            .await
            .map_err(|err| CacheError::Backend(format!("redis MDEL failed: {err}")))?;
        Ok(())
    }
}

/// Adapter around Redis Pub/Sub for invalidation stream consumption.
pub struct RedisSubscriber {
    pubsub: redis::aio::PubSub,
}

impl RedisSubscriber {
    fn new(pubsub: redis::aio::PubSub) -> Self {
        Self { pubsub }
    }
}

impl InvalidationSubscriber for RedisSubscriber {
    async fn next_message(&mut self) -> Option<CacheResult<String>> {
        let msg = {
            let mut stream = self.pubsub.on_message();
            stream.next().await
        };

        let Some(msg) = msg else {
            return None;
        };

        Some(msg.get_payload::<String>().map_err(|err| {
            CacheError::Backend(format!("redis pubsub payload decode failed: {err}"))
        }))
    }
}

impl<V> RemoteBackend<V> for RedisBackend<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    type Subscriber = RedisSubscriber;

    async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        RedisBackend::get(self, key).await
    }

    async fn mget(&self, keys: &[String]) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>> {
        RedisBackend::mget(self, keys).await
    }

    async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()> {
        RedisBackend::set(self, key, entry).await
    }

    async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()> {
        RedisBackend::mset(self, entries).await
    }

    async fn del(&self, key: &str) -> CacheResult<()> {
        RedisBackend::del(self, key).await
    }

    async fn mdel(&self, keys: &[String]) -> CacheResult<()> {
        RedisBackend::mdel(self, keys).await
    }

    async fn publish(&self, channel: &str, payload: &str) -> CacheResult<()> {
        RedisBackend::publish(self, channel, payload).await
    }

    async fn subscribe(&self, channel: &str) -> CacheResult<Self::Subscriber> {
        let pubsub = RedisBackend::subscribe(self, channel).await?;
        Ok(RedisSubscriber::new(pubsub))
    }
}

/// Builder for `RedisBackend`.
#[derive(Clone, Debug)]
pub struct RedisBuilder<V> {
    /// Redis connection URL.
    url: String,
    /// Optional key prefix.
    key_prefix: String,
    /// Type marker.
    _marker: PhantomData<V>,
}

impl<V> Default for RedisBuilder<V> {
    /// Returns default redis builder.
    fn default() -> Self {
        Self {
            url: "redis://127.0.0.1:6379".to_string(),
            key_prefix: String::new(),
            _marker: PhantomData,
        }
    }
}

impl<V> RedisBuilder<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Sets redis URL.
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Sets key prefix used by this backend.
    pub fn key_prefix(mut self, key_prefix: impl Into<String>) -> Self {
        self.key_prefix = key_prefix.into();
        self
    }

    /// Builds redis backend and validates URL format.
    pub fn build(self) -> CacheResult<RedisBackend<V>> {
        let client = redis::Client::open(self.url)
            .map_err(|err| CacheError::InvalidConfig(format!("invalid redis URL: {err}")))?;
        Ok(RedisBackend::new(client, self.key_prefix))
    }
}

/// Returns a redis backend builder.
pub fn redis<V>() -> RedisBuilder<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    RedisBuilder::default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use redis::AsyncCommands;
    use tokio::time;

    use crate::backend::{InvalidationSubscriber, RemoteBackend, StoredEntry, StoredValue};
    use crate::remote;

    fn redis_url() -> String {
        std::env::var("ACCELERATOR_TEST_REDIS_URL")
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
    }

    fn unique_scope(tag: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{tag}-{}-{nanos}", std::process::id())
    }

    async fn redis_ready(url: &str) -> bool {
        let client = match redis::Client::open(url) {
            Ok(client) => client,
            Err(_) => return false,
        };

        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(conn) => conn,
            Err(_) => return false,
        };

        conn.ping::<String>().await.is_ok()
    }

    async fn redis_backend_or_skip(test_name: &str) -> Option<remote::RedisBackend<String>> {
        let url = redis_url();
        if !redis_ready(&url).await {
            eprintln!("skip `{test_name}`: redis is not reachable at {url}");
            return None;
        }

        let backend = remote::redis::<String>()
            .url(url)
            .key_prefix(unique_scope(test_name))
            .build()
            .unwrap();
        Some(backend)
    }

    #[test]
    fn redis_builder_validates_url() {
        let backend = remote::redis::<String>().url("not-a-redis-url").build();
        assert!(backend.is_err());
    }

    #[tokio::test]
    async fn redis_backend_roundtrip_and_batch_ops() {
        let Some(backend) = redis_backend_or_skip("redis_backend_roundtrip_and_batch_ops").await
        else {
            return;
        };

        backend
            .set(
                "k1",
                StoredEntry {
                    value: StoredValue::Value("v1".to_string()),
                    expire_at: Instant::now() + Duration::from_secs(30),
                },
            )
            .await
            .unwrap();
        backend
            .set(
                "k_null",
                StoredEntry {
                    value: StoredValue::Null,
                    expire_at: Instant::now() + Duration::from_secs(30),
                },
            )
            .await
            .unwrap();

        let mut batch_entries = HashMap::new();
        batch_entries.insert(
            "k2".to_string(),
            StoredEntry {
                value: StoredValue::Value("v2".to_string()),
                expire_at: Instant::now() + Duration::from_secs(30),
            },
        );
        batch_entries.insert(
            "k3".to_string(),
            StoredEntry {
                value: StoredValue::Null,
                expire_at: Instant::now() + Duration::from_secs(30),
            },
        );
        backend.mset(batch_entries).await.unwrap();

        let value = backend.get("k1").await.unwrap().unwrap();
        assert_eq!(value.value, StoredValue::Value("v1".to_string()));
        let null_value = backend.get("k_null").await.unwrap().unwrap();
        assert_eq!(null_value.value, StoredValue::Null);

        let queried = vec!["k2".to_string(), "k3".to_string(), "k_missing".to_string()];
        let values = backend.mget(&queried).await.unwrap();
        assert_eq!(values.len(), 3);
        assert_eq!(
            values.get("k2").cloned().flatten().map(|entry| entry.value),
            Some(StoredValue::Value("v2".to_string()))
        );
        assert_eq!(
            values.get("k3").cloned().flatten().map(|entry| entry.value),
            Some(StoredValue::Null)
        );
        assert!(values.get("k_missing").cloned().flatten().is_none());

        backend.del("k1").await.unwrap();
        assert!(backend.get("k1").await.unwrap().is_none());
        backend
            .mdel(&["k2".to_string(), "k3".to_string(), "k_null".to_string()])
            .await
            .unwrap();
        let after_delete = backend
            .mget(&["k2".to_string(), "k3".to_string(), "k_null".to_string()])
            .await
            .unwrap();
        assert!(after_delete.values().all(|entry| entry.is_none()));
    }

    #[tokio::test]
    async fn redis_backend_publish_subscribe_roundtrip() {
        let Some(backend) =
            redis_backend_or_skip("redis_backend_publish_subscribe_roundtrip").await
        else {
            return;
        };

        let channel = unique_scope("remote-unit-channel");
        let mut subscriber =
            <remote::RedisBackend<String> as RemoteBackend<String>>::subscribe(&backend, &channel)
                .await
                .unwrap();
        time::sleep(Duration::from_millis(20)).await;

        <remote::RedisBackend<String> as RemoteBackend<String>>::publish(
            &backend,
            &channel,
            "hello-subscriber",
        )
        .await
        .unwrap();

        let received = time::timeout(Duration::from_secs(2), subscriber.next_message())
            .await
            .expect("subscriber timed out waiting for message");
        let payload_result = received.expect("subscriber stream ended unexpectedly");
        let message = payload_result.expect("subscriber returned payload decode error");
        assert_eq!(message, "hello-subscriber");
    }
}
