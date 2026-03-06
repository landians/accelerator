use std::future::Future;
use std::hash::Hash;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::cache::{KeyConverter, LevelCache};
use crate::config::{CacheConfig, CacheMode, ReadValueMode};
use crate::error::{CacheError, CacheResult};
use crate::loader::{FnLoader, MLoader, NoopLoader};
use crate::{local, remote};

/// Builder for fixed-backend `LevelCache` instances.
pub struct LevelCacheBuilder<K, V, LD = NoopLoader>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
{
    config: CacheConfig,
    local: Option<local::MokaBackend<V>>,
    remote: Option<remote::RedisBackend<V>>,
    loader: Option<LD>,
    key_converter: Option<KeyConverter<K>>,
    _marker: PhantomData<V>,
}

impl<K, V, LD> Default for LevelCacheBuilder<K, V, LD>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
{
    /// Creates a builder with default config and no backend wiring.
    fn default() -> Self {
        Self {
            config: CacheConfig::default(),
            local: None,
            remote: None,
            loader: None,
            key_converter: None,
            _marker: PhantomData,
        }
    }
}

impl<K, V> LevelCacheBuilder<K, V>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
{
    /// Creates a builder with default local(moka) + remote(redis) backends.
    pub fn with_defaults() -> CacheResult<Self> {
        Ok(Self::new()
            .local(local::moka::<V>().build()?)
            .remote(remote::redis::<V>().build()?))
    }
}

impl<K, V, LD> LevelCacheBuilder<K, V, LD>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
{
    /// Creates a new builder with default config.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the logical cache area namespace.
    pub fn area(mut self, area: impl Into<String>) -> Self {
        self.config.area = area.into();
        self
    }

    /// Selects the cache mode (Local/Remote/Both).
    pub fn mode(mut self, mode: CacheMode) -> Self {
        self.config.mode = mode;
        self
    }

    /// Sets the local moka backend instance.
    pub fn local(mut self, local: local::MokaBackend<V>) -> Self {
        self.local = Some(local);
        self
    }

    /// Sets the remote redis backend instance.
    pub fn remote(mut self, remote: remote::RedisBackend<V>) -> Self {
        self.remote = Some(remote);
        self
    }

    /// Sets local value TTL.
    pub fn local_ttl(mut self, local_ttl: Duration) -> Self {
        self.config.local_ttl = local_ttl;
        self
    }

    /// Sets remote value TTL.
    pub fn remote_ttl(mut self, remote_ttl: Duration) -> Self {
        self.config.remote_ttl = remote_ttl;
        self
    }

    /// Sets TTL for cached `None`.
    pub fn null_ttl(mut self, null_ttl: Duration) -> Self {
        self.config.null_ttl = null_ttl;
        self
    }

    /// Enables TTL jitter with ratio in `[0.0, 1.0]`.
    pub fn ttl_jitter_ratio(mut self, ratio: f64) -> Self {
        self.config.ttl_jitter_ratio = Some(ratio);
        self
    }

    /// Disables TTL jitter.
    pub fn disable_ttl_jitter(mut self) -> Self {
        self.config.ttl_jitter_ratio = None;
        self
    }

    /// Enables or disables negative-cache writes.
    pub fn cache_null_value(mut self, enabled: bool) -> Self {
        self.config.cache_null_value = enabled;
        self
    }

    /// Enables or disables singleflight miss deduplication.
    pub fn penetration_protect(mut self, enabled: bool) -> Self {
        self.config.penetration_protect = enabled;
        self
    }

    /// Sets loader timeout.
    pub fn loader_timeout(mut self, timeout: Duration) -> Self {
        self.config.loader_timeout = Some(timeout);
        self
    }

    /// Enables or disables warmup behavior.
    pub fn warmup_enabled(mut self, enabled: bool) -> Self {
        self.config.warmup_enabled = enabled;
        self
    }

    /// Sets warmup chunk size.
    pub fn warmup_batch_size(mut self, size: usize) -> Self {
        self.config.warmup_batch_size = size;
        self
    }

    /// Enables or disables refresh-ahead.
    pub fn refresh_ahead(mut self, enabled: bool) -> Self {
        self.config.refresh_ahead = enabled;
        self
    }

    /// Sets refresh-ahead window threshold.
    pub fn refresh_ahead_window(mut self, duration: Duration) -> Self {
        self.config.refresh_ahead_window = duration;
        self
    }

    /// Enables or disables stale fallback when load fails.
    pub fn stale_on_error(mut self, enabled: bool) -> Self {
        self.config.stale_on_error = enabled;
        self
    }

    /// Publishes invalidation events through Redis Pub/Sub on deletes.
    pub fn broadcast_invalidation(mut self, enabled: bool) -> Self {
        self.config.broadcast_invalidation = enabled;
        self
    }

    /// Disables loader timeout.
    pub fn disable_loader_timeout(mut self) -> Self {
        self.config.loader_timeout = None;
        self
    }

    /// Sets copy-on-read switch.
    pub fn copy_on_read(mut self, enabled: bool) -> Self {
        self.config.copy_on_read = enabled;
        self
    }

    /// Sets copy-on-write switch.
    pub fn copy_on_write(mut self, enabled: bool) -> Self {
        self.config.copy_on_write = enabled;
        self
    }

    /// Selects read value mode strategy.
    pub fn read_value_mode(mut self, mode: ReadValueMode) -> Self {
        self.config.read_value_mode = mode;
        self
    }

    /// Sets custom business-key to storage-key converter.
    pub fn key_converter<F>(mut self, converter: F) -> Self
    where
        F: Fn(&K) -> String + Send + Sync + 'static,
    {
        self.key_converter = Some(Arc::new(converter));
        self
    }

    /// Installs a custom loader implementation.
    pub fn loader<NLD>(self, loader: NLD) -> LevelCacheBuilder<K, V, NLD>
    where
        NLD: MLoader<K, V> + Send + Sync + 'static,
    {
        LevelCacheBuilder {
            config: self.config,
            local: self.local,
            remote: self.remote,
            loader: Some(loader),
            key_converter: self.key_converter,
            _marker: PhantomData,
        }
    }

    /// Installs a function-style single-key loader.
    pub fn loader_fn<F, Fut>(self, loader: F) -> LevelCacheBuilder<K, V, FnLoader<K, V, F, Fut>>
    where
        F: Fn(K) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = CacheResult<Option<V>>> + Send + 'static,
    {
        self.loader(FnLoader::new(loader))
    }

    /// Validates configuration and builds a `LevelCache`.
    pub fn build(self) -> CacheResult<LevelCache<K, V, LD>> {
        self.validate()?;

        let key_converter = self
            .key_converter
            .unwrap_or_else(|| Arc::new(|key: &K| key.to_string()));

        Ok(LevelCache::new(
            self.config,
            self.local,
            self.remote,
            self.loader,
            key_converter,
        ))
    }

    /// Performs eager validation for builder parameters.
    fn validate(&self) -> CacheResult<()> {
        if self.config.area.trim().is_empty() {
            return Err(CacheError::InvalidConfig(
                "area must not be empty".to_string(),
            ));
        }

        if self.config.local_ttl.is_zero() {
            return Err(CacheError::InvalidConfig(
                "local_ttl must be > 0".to_string(),
            ));
        }

        if self.config.remote_ttl.is_zero() {
            return Err(CacheError::InvalidConfig(
                "remote_ttl must be > 0".to_string(),
            ));
        }

        if self.config.null_ttl.is_zero() {
            return Err(CacheError::InvalidConfig(
                "null_ttl must be > 0".to_string(),
            ));
        }

        if self.config.warmup_batch_size == 0 {
            return Err(CacheError::InvalidConfig(
                "warmup_batch_size must be > 0".to_string(),
            ));
        }

        if let Some(ratio) = self.config.ttl_jitter_ratio {
            if !(0.0..=1.0).contains(&ratio) {
                return Err(CacheError::InvalidConfig(
                    "ttl_jitter_ratio must be in [0.0, 1.0]".to_string(),
                ));
            }
        }

        match self.config.mode {
            CacheMode::Local if self.local.is_none() => {
                return Err(CacheError::InvalidConfig(
                    "local backend is required when mode=Local".to_string(),
                ));
            }
            CacheMode::Remote if self.remote.is_none() => {
                return Err(CacheError::InvalidConfig(
                    "remote backend is required when mode=Remote".to_string(),
                ));
            }
            CacheMode::Both if self.local.is_none() || self.remote.is_none() => {
                return Err(CacheError::InvalidConfig(
                    "both local and remote backends are required when mode=Both".to_string(),
                ));
            }
            _ => {}
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::builder::LevelCacheBuilder;
    use crate::config::CacheMode;
    use crate::local;

    #[test]
    fn build_fails_without_required_backends() {
        let err = match LevelCacheBuilder::<u64, String>::new()
            .mode(CacheMode::Both)
            .build()
        {
            Ok(_) => panic!("expected build to fail when required backends are missing"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("both local and remote backends"));
    }

    #[test]
    fn build_fails_on_zero_ttls() {
        let local_backend = local::moka::<String>().build().unwrap();

        let err = match LevelCacheBuilder::<u64, String>::new()
            .mode(CacheMode::Local)
            .local(local_backend)
            .local_ttl(Duration::ZERO)
            .build()
        {
            Ok(_) => panic!("expected build to fail when local_ttl is zero"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("local_ttl"));
    }

    #[test]
    fn build_succeeds_with_minimum_valid_config() {
        let local_backend = local::moka::<String>().build().unwrap();

        let cache = LevelCacheBuilder::<u64, String>::new()
            .mode(CacheMode::Local)
            .local(local_backend)
            .key_converter(|k| k.to_string())
            .build();

        assert!(cache.is_ok());
    }

    #[test]
    fn build_fails_on_invalid_ttl_jitter_ratio() {
        let local_backend = local::moka::<String>().build().unwrap();

        let err = match LevelCacheBuilder::<u64, String>::new()
            .mode(CacheMode::Local)
            .local(local_backend)
            .ttl_jitter_ratio(1.5)
            .build()
        {
            Ok(_) => panic!("expected build to fail when ttl_jitter_ratio is invalid"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("ttl_jitter_ratio"));
    }

    #[test]
    fn build_fails_on_zero_warmup_batch_size() {
        let local_backend = local::moka::<String>().build().unwrap();

        let err = match LevelCacheBuilder::<u64, String>::new()
            .mode(CacheMode::Local)
            .local(local_backend)
            .warmup_batch_size(0)
            .build()
        {
            Ok(_) => panic!("expected build to fail when warmup_batch_size is zero"),
            Err(err) => err,
        };

        assert!(format!("{err}").contains("warmup_batch_size"));
    }
}
