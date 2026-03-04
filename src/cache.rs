use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde::de::DeserializeOwned;
use singleflight_async::SingleFlight;
use tokio::time;
use tracing::instrument;

use crate::backend::{StoredEntry, StoredValue};
use crate::config::{CacheConfig, CacheMode};
use crate::error::{CacheError, CacheResult};
use crate::loader::{MLoader, NoopLoader};
use crate::{local, remote};

pub type KeyConverter<K> = Arc<dyn Fn(&K) -> String + Send + Sync>;

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions {
    pub allow_stale: bool,
    pub disable_load: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheMetricsSnapshot {
    pub local_hit: u64,
    pub local_miss: u64,
    pub remote_hit: u64,
    pub remote_miss: u64,
    pub load_total: u64,
    pub load_success: u64,
    pub load_timeout: u64,
    pub load_error: u64,
}

#[derive(Debug, Default)]
struct CacheMetrics {
    local_hit: AtomicU64,
    local_miss: AtomicU64,
    remote_hit: AtomicU64,
    remote_miss: AtomicU64,
    load_total: AtomicU64,
    load_success: AtomicU64,
    load_timeout: AtomicU64,
    load_error: AtomicU64,
}

impl CacheMetrics {
    fn snapshot(&self) -> CacheMetricsSnapshot {
        CacheMetricsSnapshot {
            local_hit: self.local_hit.load(Ordering::Relaxed),
            local_miss: self.local_miss.load(Ordering::Relaxed),
            remote_hit: self.remote_hit.load(Ordering::Relaxed),
            remote_miss: self.remote_miss.load(Ordering::Relaxed),
            load_total: self.load_total.load(Ordering::Relaxed),
            load_success: self.load_success.load(Ordering::Relaxed),
            load_timeout: self.load_timeout.load(Ordering::Relaxed),
            load_error: self.load_error.load(Ordering::Relaxed),
        }
    }
}

pub struct LevelCache<K, V, LD = NoopLoader>
where
    K: Clone + Eq + std::hash::Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
{
    pub(crate) config: CacheConfig,
    pub(crate) local: Option<local::MokaBackend<V>>,
    pub(crate) remote: Option<remote::RedisBackend<V>>,
    pub(crate) loader: Option<LD>,
    pub(crate) key_converter: KeyConverter<K>,
    singleflight: Arc<SingleFlight<String, CacheResult<Option<V>>>>,
    metrics: Arc<CacheMetrics>,
}

impl<K, V, LD> LevelCache<K, V, LD>
where
    K: Clone + Eq + std::hash::Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
{
    pub(crate) fn new(
        config: CacheConfig,
        local: Option<local::MokaBackend<V>>,
        remote: Option<remote::RedisBackend<V>>,
        loader: Option<LD>,
        key_converter: KeyConverter<K>,
    ) -> Self {
        Self {
            config,
            local,
            remote,
            loader,
            key_converter,
            singleflight: Arc::new(SingleFlight::new()),
            metrics: Arc::new(CacheMetrics::default()),
        }
    }

    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    pub fn metrics_snapshot(&self) -> CacheMetricsSnapshot {
        self.metrics.snapshot()
    }

    #[instrument(name = "cache.get", skip_all, fields(area = %self.config.area))]
    pub async fn get(&self, key: &K, options: &ReadOptions) -> CacheResult<Option<V>> {
        let encoded_key = self.encoded_key(key);

        if let Some(value) = self.read_local_value(&encoded_key).await? {
            return Ok(value);
        }

        if let Some(value) = self
            .read_remote_value_and_backfill_local(&encoded_key)
            .await?
        {
            return Ok(value);
        }

        if options.disable_load || self.loader.is_none() {
            return Ok(None);
        }

        let _ = options.allow_stale;
        self.load_on_miss(key.clone(), encoded_key).await
    }

    #[instrument(name = "cache.mget", skip_all, fields(area = %self.config.area, size = keys.len()))]
    pub async fn mget(
        &self,
        keys: &[K],
        options: &ReadOptions,
    ) -> CacheResult<HashMap<K, Option<V>>> {
        let mut values = HashMap::with_capacity(keys.len());
        let encoded_pairs = keys
            .iter()
            .map(|key| (key.clone(), self.encoded_key(key)))
            .collect::<Vec<_>>();

        let mut misses = self.fill_from_local(&encoded_pairs, &mut values).await?;
        misses = self
            .fill_from_remote_and_backfill_local(misses, &mut values)
            .await?;

        if misses.is_empty() || options.disable_load || self.loader.is_none() {
            for (key, _) in misses {
                values.insert(key, None);
            }
            return Ok(values);
        }

        if self.config.penetration_protect {
            for (key, encoded_key) in misses {
                let value = self.load_on_miss(key.clone(), encoded_key).await?;
                values.insert(key, value);
            }
            return Ok(values);
        }

        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| CacheError::InvalidConfig("loader is not configured".to_string()))?;

        let request_keys = misses
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();

        let loaded_map = if let Some(timeout) = self.config.loader_timeout {
            match time::timeout(timeout, loader.mload(&request_keys)).await {
                Ok(value) => value,
                Err(_) => return Err(CacheError::Timeout("loader")),
            }
        } else {
            loader.mload(&request_keys).await
        }?;

        for (key, encoded_key) in misses {
            let loaded = loaded_map.get(&key).cloned().unwrap_or(None);
            self.write_by_mode(&encoded_key, loaded.clone()).await?;
            values.insert(key, loaded);
        }

        Ok(values)
    }

    #[instrument(name = "cache.set", skip_all, fields(area = %self.config.area))]
    pub async fn set(&self, key: &K, value: Option<V>) -> CacheResult<()> {
        let encoded_key = self.encoded_key(key);
        self.write_by_mode(&encoded_key, value).await
    }

    #[instrument(name = "cache.mset", skip_all, fields(area = %self.config.area, size = entries.len()))]
    pub async fn mset(&self, entries: HashMap<K, Option<V>>) -> CacheResult<()> {
        for (key, value) in entries {
            self.set(&key, value).await?;
        }
        Ok(())
    }

    #[instrument(name = "cache.del", skip_all, fields(area = %self.config.area))]
    pub async fn del(&self, key: &K) -> CacheResult<()> {
        let encoded_key = self.encoded_key(key);
        self.delete_remote_if_needed(&encoded_key).await?;
        self.delete_local_if_needed(&encoded_key).await?;
        Ok(())
    }

    #[instrument(name = "cache.mdel", skip_all, fields(area = %self.config.area, size = keys.len()))]
    pub async fn mdel(&self, keys: &[K]) -> CacheResult<()> {
        if keys.is_empty() {
            return Ok(());
        }

        let encoded = keys
            .iter()
            .map(|key| self.encoded_key(key))
            .collect::<Vec<_>>();

        self.batch_delete_remote_if_needed(&encoded).await?;
        self.batch_delete_local_if_needed(&encoded).await?;
        Ok(())
    }

    fn encoded_key(&self, key: &K) -> String {
        format!("{}:{}", self.config.area, (self.key_converter)(key))
    }

    fn mode_uses_local(&self) -> bool {
        matches!(self.config.mode, CacheMode::Local | CacheMode::Both)
    }

    fn mode_uses_remote(&self) -> bool {
        matches!(self.config.mode, CacheMode::Remote | CacheMode::Both)
    }

    async fn read_local_value(&self, encoded_key: &str) -> CacheResult<Option<Option<V>>> {
        if !self.mode_uses_local() {
            return Ok(None);
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(None);
        };

        let entry = local.get(encoded_key).await?;
        if entry.is_some() {
            self.metrics.local_hit.fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics.local_miss.fetch_add(1, Ordering::Relaxed);
        }
        Ok(entry.map(Self::entry_to_value))
    }

    async fn read_remote_value_and_backfill_local(
        &self,
        encoded_key: &str,
    ) -> CacheResult<Option<Option<V>>> {
        if !self.mode_uses_remote() {
            return Ok(None);
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(None);
        };

        let Some(entry) = remote.get(encoded_key).await? else {
            self.metrics.remote_miss.fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        };

        self.metrics.remote_hit.fetch_add(1, Ordering::Relaxed);
        let value = Self::entry_to_value(entry);
        self.backfill_local_if_needed(encoded_key, value.clone())
            .await?;
        Ok(Some(value))
    }

    async fn backfill_local_if_needed(
        &self,
        encoded_key: &str,
        value: Option<V>,
    ) -> CacheResult<()> {
        if !self.mode_uses_local() {
            return Ok(());
        }
        self.backfill_local(encoded_key, value).await
    }

    async fn delete_remote_if_needed(&self, encoded_key: &str) -> CacheResult<()> {
        if !self.mode_uses_remote() {
            return Ok(());
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };

        remote.del(encoded_key).await
    }

    async fn delete_local_if_needed(&self, encoded_key: &str) -> CacheResult<()> {
        if !self.mode_uses_local() {
            return Ok(());
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(());
        };

        local.del(encoded_key).await
    }

    async fn batch_delete_remote_if_needed(&self, encoded_keys: &[String]) -> CacheResult<()> {
        if !self.mode_uses_remote() {
            return Ok(());
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };

        remote.mdel(encoded_keys).await
    }

    async fn batch_delete_local_if_needed(&self, encoded_keys: &[String]) -> CacheResult<()> {
        if !self.mode_uses_local() {
            return Ok(());
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(());
        };

        local.mdel(encoded_keys).await
    }

    async fn fill_from_local(
        &self,
        encoded_pairs: &[(K, String)],
        values: &mut HashMap<K, Option<V>>,
    ) -> CacheResult<Vec<(K, String)>> {
        if !self.mode_uses_local() {
            return Ok(encoded_pairs.to_vec());
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(encoded_pairs.to_vec());
        };
        let keys = encoded_pairs
            .iter()
            .map(|(_, encoded)| encoded.clone())
            .collect::<Vec<_>>();
        let local_values = local.mget(&keys).await?;

        let mut misses = Vec::new();
        for (key, encoded_key) in encoded_pairs {
            match local_values.get(encoded_key).cloned().flatten() {
                Some(entry) => {
                    self.metrics.local_hit.fetch_add(1, Ordering::Relaxed);
                    values.insert(key.clone(), Self::entry_to_value(entry));
                }
                None => {
                    self.metrics.local_miss.fetch_add(1, Ordering::Relaxed);
                    misses.push((key.clone(), encoded_key.clone()));
                }
            }
        }

        Ok(misses)
    }

    async fn fill_from_remote_and_backfill_local(
        &self,
        misses: Vec<(K, String)>,
        values: &mut HashMap<K, Option<V>>,
    ) -> CacheResult<Vec<(K, String)>> {
        if misses.is_empty() || !self.mode_uses_remote() {
            return Ok(misses);
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(misses);
        };
        let keys = misses
            .iter()
            .map(|(_, encoded)| encoded.clone())
            .collect::<Vec<_>>();
        let remote_values = remote.mget(&keys).await?;

        let can_backfill_local = self.mode_uses_local() && self.local.is_some();
        let mut backfill_entries = HashMap::new();
        let mut remained = Vec::new();

        for (key, encoded_key) in misses {
            match remote_values.get(&encoded_key).cloned().flatten() {
                Some(entry) => {
                    self.metrics.remote_hit.fetch_add(1, Ordering::Relaxed);
                    let value = Self::entry_to_value(entry);
                    if can_backfill_local {
                        backfill_entries.insert(
                            encoded_key.clone(),
                            Self::to_entry(
                                &encoded_key,
                                value.clone(),
                                self.config.local_ttl,
                                self.config.null_ttl,
                                self.config.ttl_jitter_ratio,
                            ),
                        );
                    }
                    values.insert(key, value);
                }
                None => {
                    self.metrics.remote_miss.fetch_add(1, Ordering::Relaxed);
                    remained.push((key, encoded_key));
                }
            }
        }

        if backfill_entries.is_empty() {
            return Ok(remained);
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(remained);
        };
        local.mset(backfill_entries).await?;

        Ok(remained)
    }

    async fn backfill_local(&self, encoded_key: &str, value: Option<V>) -> CacheResult<()> {
        let Some(local) = self.local.as_ref() else {
            return Ok(());
        };

        local
            .set(
                encoded_key,
                self.new_entry(encoded_key, value, self.config.local_ttl),
            )
            .await
    }

    #[instrument(name = "cache.load", skip_all, fields(area = %self.config.area))]
    async fn load_on_miss(&self, key: K, encoded_key: String) -> CacheResult<Option<V>> {
        if self.loader.is_none() {
            return Err(CacheError::InvalidConfig(
                "loader is not configured".to_string(),
            ));
        }

        if !self.config.penetration_protect {
            return self.load_and_write(&key, &encoded_key).await;
        }

        self.singleflight
            .work(encoded_key.clone(), || async {
                self.load_and_write(&key, &encoded_key).await
            })
            .await
    }

    async fn load_and_write(&self, key: &K, encoded_key: &str) -> CacheResult<Option<V>> {
        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| CacheError::InvalidConfig("loader is not configured".to_string()))?;

        self.metrics.load_total.fetch_add(1, Ordering::Relaxed);
        let load_future = loader.load(key);
        let loaded = if let Some(timeout) = self.config.loader_timeout {
            match time::timeout(timeout, load_future).await {
                Ok(value) => value,
                Err(_) => {
                    self.metrics.load_timeout.fetch_add(1, Ordering::Relaxed);
                    return Err(CacheError::Timeout("loader"));
                }
            }
        } else {
            load_future.await
        };

        let loaded = match loaded {
            Ok(value) => value,
            Err(err) => {
                self.metrics.load_error.fetch_add(1, Ordering::Relaxed);
                return Err(err);
            }
        };

        self.metrics.load_success.fetch_add(1, Ordering::Relaxed);
        self.write_by_mode(encoded_key, loaded.clone()).await?;
        Ok(loaded)
    }

    async fn write_by_mode(&self, encoded_key: &str, value: Option<V>) -> CacheResult<()> {
        match self.config.mode {
            CacheMode::Local => {
                Self::write_local(&self.local, &self.config, encoded_key, value).await
            }
            CacheMode::Remote => {
                Self::write_remote(&self.remote, &self.config, encoded_key, value).await
            }
            CacheMode::Both => {
                Self::write_remote(&self.remote, &self.config, encoded_key, value.clone()).await?;
                Self::write_local(&self.local, &self.config, encoded_key, value).await
            }
        }
    }

    async fn write_local(
        local: &Option<local::MokaBackend<V>>,
        config: &CacheConfig,
        encoded_key: &str,
        value: Option<V>,
    ) -> CacheResult<()> {
        let Some(local) = local.as_ref() else {
            return Ok(());
        };

        if value.is_none() && !config.cache_null_value {
            return local.del(encoded_key).await;
        }

        local
            .set(
                encoded_key,
                Self::to_entry(
                    encoded_key,
                    value,
                    config.local_ttl,
                    config.null_ttl,
                    config.ttl_jitter_ratio,
                ),
            )
            .await
    }

    async fn write_remote(
        remote: &Option<remote::RedisBackend<V>>,
        config: &CacheConfig,
        encoded_key: &str,
        value: Option<V>,
    ) -> CacheResult<()> {
        let Some(remote) = remote.as_ref() else {
            return Ok(());
        };

        if value.is_none() && !config.cache_null_value {
            return remote.del(encoded_key).await;
        }

        remote
            .set(
                encoded_key,
                Self::to_entry(
                    encoded_key,
                    value,
                    config.remote_ttl,
                    config.null_ttl,
                    config.ttl_jitter_ratio,
                ),
            )
            .await
    }

    fn entry_to_value(entry: StoredEntry<V>) -> Option<V> {
        match entry.value {
            StoredValue::Value(value) => Some(value),
            StoredValue::Null => None,
        }
    }

    fn new_entry(
        &self,
        encoded_key: &str,
        value: Option<V>,
        ttl_for_value: Duration,
    ) -> StoredEntry<V> {
        Self::to_entry(
            encoded_key,
            value,
            ttl_for_value,
            self.config.null_ttl,
            self.config.ttl_jitter_ratio,
        )
    }

    fn jitter_ttl(encoded_key: &str, base_ttl: Duration, ratio: Option<f64>) -> Duration {
        let Some(ratio) = ratio else {
            return base_ttl;
        };

        if !(0.0..=1.0).contains(&ratio) || ratio == 0.0 {
            return base_ttl;
        }

        let base_ms = base_ttl.as_millis().max(1);
        let max_jitter = ((base_ms as f64) * ratio).round() as u128;
        if max_jitter == 0 {
            return base_ttl;
        }

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        encoded_key.hash(&mut hasher);
        let range = max_jitter.saturating_mul(2).saturating_add(1);
        let slot = (hasher.finish() as u128) % range;
        let jitter = slot as i128 - max_jitter as i128;

        let jittered_ms = (base_ms as i128 + jitter).max(1) as u128;
        let capped_ms = jittered_ms.min(u64::MAX as u128);
        Duration::from_millis(capped_ms as u64)
    }

    fn to_entry(
        encoded_key: &str,
        value: Option<V>,
        ttl_for_value: Duration,
        ttl_for_null: Duration,
        jitter_ratio: Option<f64>,
    ) -> StoredEntry<V> {
        let ttl = if value.is_some() {
            ttl_for_value
        } else {
            ttl_for_null
        };
        let ttl = Self::jitter_ttl(encoded_key, ttl, jitter_ratio);

        StoredEntry {
            value: match value {
                Some(v) => StoredValue::Value(v),
                None => StoredValue::Null,
            },
            expire_at: Instant::now() + ttl,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use futures_util::future::join_all;

    use crate::builder::LevelCacheBuilder;
    use crate::cache::ReadOptions;
    use crate::config::CacheMode;
    use crate::local;

    type TestBuilder = LevelCacheBuilder<u64, String>;

    fn test_cache_builder() -> TestBuilder {
        let local_backend = local::moka::<String>().max_capacity(1024).build().unwrap();

        LevelCacheBuilder::new()
            .area("test")
            .mode(CacheMode::Local)
            .local(local_backend)
            .local_ttl(Duration::from_secs(60))
            .remote_ttl(Duration::from_secs(120))
            .null_ttl(Duration::from_secs(10))
    }

    #[tokio::test]
    async fn get_hits_local_directly() {
        let cache = test_cache_builder().build().unwrap();
        cache.set(&1, Some("alpha".to_string())).await.unwrap();

        let value = cache.get(&1, &Default::default()).await.unwrap();
        assert_eq!(value, Some("alpha".to_string()));
    }

    #[tokio::test]
    async fn miss_without_loader_returns_none() {
        let cache = test_cache_builder().build().unwrap();
        let value = cache.get(&999, &Default::default()).await.unwrap();
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn loader_runs_and_persists_data() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .loader_fn(move |key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(format!("user-{key}")))
                }
            })
            .build()
            .unwrap();

        let first = cache.get(&7, &Default::default()).await.unwrap();
        let second = cache.get(&7, &Default::default()).await.unwrap();

        assert_eq!(first, Some("user-7".to_string()));
        assert_eq!(second, Some("user-7".to_string()));
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn null_cache_prevents_repeated_loads() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .loader_fn(move |_key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    Ok(None)
                }
            })
            .build()
            .unwrap();

        let a = cache.get(&8, &Default::default()).await.unwrap();
        let b = cache.get(&8, &Default::default()).await.unwrap();

        assert_eq!(a, None);
        assert_eq!(b, None);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn disable_load_option_skips_loader() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .loader_fn(move |_key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    Ok(Some("will-not-run".to_string()))
                }
            })
            .build()
            .unwrap();

        let value = cache
            .get(
                &55,
                &ReadOptions {
                    allow_stale: false,
                    disable_load: true,
                },
            )
            .await
            .unwrap();

        assert_eq!(value, None);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn singleflight_deduplicates_concurrent_misses() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = Arc::new(
            test_cache_builder()
                .loader_fn(move |key: u64| {
                    let loads_for_loader = loads_for_loader.clone();
                    async move {
                        loads_for_loader.fetch_add(1, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(25)).await;
                        Ok(Some(format!("key-{key}")))
                    }
                })
                .build()
                .unwrap(),
        );

        let tasks = (0..32)
            .map(|_| {
                let cache = cache.clone();
                tokio::spawn(async move { cache.get(&9, &Default::default()).await })
            })
            .collect::<Vec<_>>();

        let all = join_all(tasks).await;
        for result in all {
            let value = result.unwrap().unwrap();
            assert_eq!(value, Some("key-9".to_string()));
        }

        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn mget_returns_values_for_all_requested_keys() {
        let cache = test_cache_builder().build().unwrap();
        cache.set(&1, Some("one".to_string())).await.unwrap();
        cache.set(&2, Some("two".to_string())).await.unwrap();

        let values = cache.mget(&[1, 2, 3], &Default::default()).await.unwrap();

        assert_eq!(values.len(), 3);
        assert_eq!(values.get(&1).cloned().flatten(), Some("one".to_string()));
        assert_eq!(values.get(&2).cloned().flatten(), Some("two".to_string()));
        assert_eq!(values.get(&3).cloned().flatten(), None);
    }

    #[tokio::test]
    async fn del_and_mdel_clear_entries() {
        let cache = test_cache_builder().build().unwrap();
        cache.set(&1, Some("one".to_string())).await.unwrap();
        cache.set(&2, Some("two".to_string())).await.unwrap();

        cache.del(&1).await.unwrap();
        assert_eq!(cache.get(&1, &Default::default()).await.unwrap(), None);

        cache.mdel(&[2]).await.unwrap();
        assert_eq!(cache.get(&2, &Default::default()).await.unwrap(), None);
    }

    #[tokio::test]
    async fn loader_timeout_is_reported() {
        let cache = test_cache_builder()
            .loader_timeout(Duration::from_millis(10))
            .loader_fn(|_key: u64| async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(Some("late".to_string()))
            })
            .build()
            .unwrap();

        let err = cache.get(&1, &Default::default()).await.unwrap_err();
        assert!(matches!(err, crate::error::CacheError::Timeout("loader")));
        let metrics = cache.metrics_snapshot();
        assert_eq!(metrics.load_total, 1);
        assert_eq!(metrics.load_timeout, 1);
    }

    #[tokio::test]
    async fn ttl_jitter_spreads_expire_time() {
        let local_backend = local::moka::<String>().build().unwrap();
        let cache = LevelCacheBuilder::<u64, String>::new()
            .area("test")
            .mode(CacheMode::Local)
            .local(local_backend.clone())
            .local_ttl(Duration::from_secs(1))
            .null_ttl(Duration::from_secs(1))
            .ttl_jitter_ratio(0.5)
            .build()
            .unwrap();

        cache.set(&11, Some("a".to_string())).await.unwrap();
        cache.set(&22, Some("b".to_string())).await.unwrap();

        let now = Instant::now();
        let e1 = local_backend.get("test:11").await.unwrap().unwrap();
        let e2 = local_backend.get("test:22").await.unwrap().unwrap();
        let r1_ms = e1.expire_at.saturating_duration_since(now).as_millis();
        let r2_ms = e2.expire_at.saturating_duration_since(now).as_millis();

        assert!((300..=1600).contains(&r1_ms));
        assert!((300..=1600).contains(&r2_ms));
        assert_ne!(e1.expire_at, e2.expire_at);
    }

    #[tokio::test]
    async fn metrics_snapshot_tracks_hits_misses_and_loads() {
        let cache = test_cache_builder()
            .loader_fn(|key: u64| async move { Ok(Some(format!("v-{key}"))) })
            .build()
            .unwrap();

        let _ = cache.get(&1, &ReadOptions::default()).await.unwrap();
        let _ = cache.get(&1, &ReadOptions::default()).await.unwrap();
        let _ = cache
            .get(
                &2,
                &ReadOptions {
                    allow_stale: false,
                    disable_load: true,
                },
            )
            .await
            .unwrap();

        let metrics = cache.metrics_snapshot();
        assert!(metrics.local_hit >= 1);
        assert!(metrics.local_miss >= 2);
        assert_eq!(metrics.load_total, 1);
        assert_eq!(metrics.load_success, 1);
        assert_eq!(metrics.load_timeout, 0);
    }

    #[tokio::test]
    async fn metrics_snapshot_tracks_loader_errors() {
        let cache = test_cache_builder()
            .loader_fn(|_key: u64| async move {
                Err(crate::error::CacheError::Loader("boom".to_string()))
            })
            .build()
            .unwrap();

        let err = cache.get(&10, &ReadOptions::default()).await.unwrap_err();
        assert!(matches!(err, crate::error::CacheError::Loader(_)));

        let metrics = cache.metrics_snapshot();
        assert_eq!(metrics.load_total, 1);
        assert_eq!(metrics.load_error, 1);
        assert_eq!(metrics.load_success, 0);
    }
}
