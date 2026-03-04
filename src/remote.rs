use std::collections::HashMap;
use std::marker::PhantomData;
use std::time::{Duration, Instant};

use redis::AsyncCommands;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::backend::{StoredEntry, StoredValue};
use crate::error::{CacheError, CacheResult};

// Redis GET/MGET responses do not include per-key TTL metadata.
// This placeholder keeps StoredEntry non-expired for value transport only.
// Real expiration remains enforced by Redis through PSETEX at write time.
const REDIS_READ_PLACEHOLDER_TTL: Duration = Duration::from_secs(3600);

#[derive(Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
enum RedisStoredValue<V> {
    Value(V),
    Null,
}

#[derive(Clone)]
pub struct RedisBackend<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    client: redis::Client,
    key_prefix: String,
    _marker: PhantomData<V>,
}

impl<V> RedisBackend<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    fn new(client: redis::Client, key_prefix: String) -> Self {
        Self {
            client,
            key_prefix,
            _marker: PhantomData,
        }
    }

    fn prefixed_key(&self, key: &str) -> String {
        if self.key_prefix.is_empty() {
            key.to_string()
        } else {
            format!("{}:{}", self.key_prefix, key)
        }
    }

    fn to_payload(value: StoredValue<V>) -> CacheResult<Vec<u8>> {
        let payload = match value {
            StoredValue::Value(value) => RedisStoredValue::Value(value),
            StoredValue::Null => RedisStoredValue::Null,
        };

        serde_json::to_vec(&payload)
            .map_err(|err| CacheError::Backend(format!("serialize redis payload failed: {err}")))
    }

    fn from_payload(bytes: &[u8]) -> CacheResult<StoredValue<V>> {
        let value: RedisStoredValue<V> = serde_json::from_slice(bytes).map_err(|err| {
            CacheError::Backend(format!("deserialize redis payload failed: {err}"))
        })?;

        Ok(match value {
            RedisStoredValue::Value(value) => StoredValue::Value(value),
            RedisStoredValue::Null => StoredValue::Null,
        })
    }

    fn ttl_ms(expire_at: Instant) -> u64 {
        expire_at
            .saturating_duration_since(Instant::now())
            .as_millis()
            .max(1) as u64
    }

    async fn connection(&self) -> CacheResult<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|err| CacheError::Backend(format!("redis connection failed: {err}")))
    }

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

    pub async fn del(&self, key: &str) -> CacheResult<()> {
        let full_key = self.prefixed_key(key);
        let mut conn = self.connection().await?;
        let _: usize = conn
            .del(&full_key)
            .await
            .map_err(|err| CacheError::Backend(format!("redis DEL failed: {err}")))?;
        Ok(())
    }

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

#[derive(Clone, Debug)]
pub struct RedisBuilder<V> {
    url: String,
    key_prefix: String,
    _marker: PhantomData<V>,
}

impl<V> Default for RedisBuilder<V> {
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
    pub fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    pub fn key_prefix(mut self, key_prefix: impl Into<String>) -> Self {
        self.key_prefix = key_prefix.into();
        self
    }

    pub fn build(self) -> CacheResult<RedisBackend<V>> {
        let client = redis::Client::open(self.url)
            .map_err(|err| CacheError::InvalidConfig(format!("invalid redis URL: {err}")))?;
        Ok(RedisBackend::new(client, self.key_prefix))
    }
}

pub fn redis<V>() -> RedisBuilder<V>
where
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    RedisBuilder::default()
}

#[cfg(test)]
mod tests {
    use crate::remote;

    #[test]
    fn redis_builder_validates_url() {
        let backend = remote::redis::<String>().url("not-a-redis-url").build();
        assert!(backend.is_err());
    }
}
