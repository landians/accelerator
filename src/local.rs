use std::collections::HashMap;
use std::marker::PhantomData;

use moka::future::Cache;

use crate::backend::{LocalBackend, StoredEntry};
use crate::error::CacheResult;

/// Local in-memory backend implemented by `moka::future::Cache`.
#[derive(Clone)]
pub struct MokaBackend<V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Underlying moka cache storage.
    cache: Cache<String, StoredEntry<V>>,
}

impl<V> MokaBackend<V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Creates a new moka backend with bounded capacity.
    fn new(max_capacity: u64) -> Self {
        let cache = Cache::builder().max_capacity(max_capacity).build();
        Self { cache }
    }

    /// Reads one key and performs eager expiration check.
    pub async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        let Some(entry) = self.cache.get(key).await else {
            return Ok(None);
        };

        if entry.is_expired() {
            self.cache.invalidate(key).await;
            return Ok(None);
        }

        Ok(Some(entry))
    }

    /// Reads one key without expiration filtering (used for stale fallback).
    pub async fn peek(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        Ok(self.cache.get(key).await)
    }

    /// Reads multiple keys and returns a map of optional entries.
    pub async fn mget(
        &self,
        keys: &[String],
    ) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>> {
        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(key.clone(), self.get(key).await?);
        }
        Ok(values)
    }

    /// Writes one key/value entry.
    pub async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()> {
        self.cache.insert(key.to_string(), entry).await;
        Ok(())
    }

    /// Writes multiple entries sequentially.
    pub async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()> {
        for (key, entry) in entries {
            self.set(&key, entry).await?;
        }
        Ok(())
    }

    /// Deletes one key.
    pub async fn del(&self, key: &str) -> CacheResult<()> {
        self.cache.invalidate(key).await;
        Ok(())
    }

    /// Deletes multiple keys.
    pub async fn mdel(&self, keys: &[String]) -> CacheResult<()> {
        for key in keys {
            self.del(key).await?;
        }
        Ok(())
    }
}

impl<V> LocalBackend<V> for MokaBackend<V>
where
    V: Clone + Send + Sync + 'static,
{
    async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        MokaBackend::get(self, key).await
    }

    async fn peek(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        MokaBackend::peek(self, key).await
    }

    async fn mget(&self, keys: &[String]) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>> {
        MokaBackend::mget(self, keys).await
    }

    async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()> {
        MokaBackend::set(self, key, entry).await
    }

    async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()> {
        MokaBackend::mset(self, entries).await
    }

    async fn del(&self, key: &str) -> CacheResult<()> {
        MokaBackend::del(self, key).await
    }

    async fn mdel(&self, keys: &[String]) -> CacheResult<()> {
        MokaBackend::mdel(self, keys).await
    }
}

/// Builder for `MokaBackend`.
#[derive(Clone, Debug)]
pub struct MokaBuilder<V> {
    /// Max entry count accepted by moka.
    max_capacity: u64,
    /// Type marker.
    _marker: PhantomData<V>,
}

impl<V> Default for MokaBuilder<V> {
    /// Returns default moka builder.
    fn default() -> Self {
        Self {
            max_capacity: 100_000,
            _marker: PhantomData,
        }
    }
}

impl<V> MokaBuilder<V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Sets max capacity, with minimum value clamped to `1`.
    pub fn max_capacity(mut self, max_capacity: u64) -> Self {
        self.max_capacity = max_capacity.max(1);
        self
    }

    /// Builds a moka backend instance.
    pub fn build(self) -> CacheResult<MokaBackend<V>> {
        Ok(MokaBackend::new(self.max_capacity))
    }
}

/// Returns a moka backend builder.
pub fn moka<V>() -> MokaBuilder<V>
where
    V: Clone + Send + Sync + 'static,
{
    MokaBuilder::default()
}

#[cfg(test)]
mod tests {
    use crate::backend::{StoredEntry, StoredValue};
    use crate::local;

    #[tokio::test]
    async fn moka_backend_can_roundtrip_value() {
        let backend = local::moka::<String>().max_capacity(16).build().unwrap();
        backend
            .set(
                "k1",
                StoredEntry {
                    value: StoredValue::Value("v1".to_string()),
                    expire_at: std::time::Instant::now() + std::time::Duration::from_secs(10),
                },
            )
            .await
            .unwrap();

        let value = backend.get("k1").await.unwrap();
        assert!(value.is_some());
    }
}
