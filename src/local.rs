use std::collections::HashMap;
use std::marker::PhantomData;

use moka::future::Cache;

use crate::backend::StoredEntry;
use crate::error::CacheResult;

#[derive(Clone)]
pub struct MokaBackend<V>
where
    V: Clone + Send + Sync + 'static,
{
    cache: Cache<String, StoredEntry<V>>,
}

impl<V> MokaBackend<V>
where
    V: Clone + Send + Sync + 'static,
{
    fn new(max_capacity: u64) -> Self {
        let cache = Cache::builder().max_capacity(max_capacity).build();
        Self { cache }
    }

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

    pub async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()> {
        self.cache.insert(key.to_string(), entry).await;
        Ok(())
    }

    pub async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()> {
        for (key, entry) in entries {
            self.set(&key, entry).await?;
        }
        Ok(())
    }

    pub async fn del(&self, key: &str) -> CacheResult<()> {
        self.cache.invalidate(key).await;
        Ok(())
    }

    pub async fn mdel(&self, keys: &[String]) -> CacheResult<()> {
        for key in keys {
            self.del(key).await?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct MokaBuilder<V> {
    max_capacity: u64,
    _marker: PhantomData<V>,
}

impl<V> Default for MokaBuilder<V> {
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
    pub fn max_capacity(mut self, max_capacity: u64) -> Self {
        self.max_capacity = max_capacity.max(1);
        self
    }

    pub fn build(self) -> CacheResult<MokaBackend<V>> {
        Ok(MokaBackend::new(self.max_capacity))
    }
}

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
