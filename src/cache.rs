use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;
use singleflight_async::SingleFlight;
use tokio::time;
use tokio::time::MissedTickBehavior;
use tracing::instrument;

use crate::backend::{
    InvalidationSubscriber, LocalBackend, RemoteBackend, StoredEntry, StoredValue,
};
use crate::config::{CacheConfig, CacheMode};
use crate::error::{CacheError, CacheResult};
use crate::loader::{MLoader, NoopLoader};
use crate::{local, remote};

/// Converts business key `K` to encoded storage key suffix.
pub type KeyConverter<K> = Arc<dyn Fn(&K) -> String + Send + Sync>;

/// Per-request read switches.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOptions {
    /// Allows stale return on load error when `stale_on_error` is enabled.
    pub allow_stale: bool,
    /// Skips loader even if cache misses.
    pub disable_load: bool,
}

/// Aggregated runtime metrics snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheMetricsSnapshot {
    /// Local cache hit count.
    pub local_hit: u64,
    /// Local cache miss count.
    pub local_miss: u64,
    /// Remote cache hit count.
    pub remote_hit: u64,
    /// Remote cache miss count.
    pub remote_miss: u64,
    /// Total load attempts.
    pub load_total: u64,
    /// Successful load count.
    pub load_success: u64,
    /// Timed-out load count.
    pub load_timeout: u64,
    /// Failed load count.
    pub load_error: u64,
    /// Returned stale value because fresh load failed.
    pub stale_fallback: u64,
    /// Refresh-ahead attempt count.
    pub refresh_attempts: u64,
    /// Refresh-ahead success count.
    pub refresh_success: u64,
    /// Refresh-ahead failure count.
    pub refresh_failures: u64,
    /// Invalidation publish attempts.
    pub invalidation_publish: u64,
    /// Invalidation publish failures.
    pub invalidation_publish_failures: u64,
    /// Invalidation events applied from subscriber.
    pub invalidation_receive: u64,
    /// Invalidation subscribe/consume failures.
    pub invalidation_receive_failures: u64,
}

/// Read-only runtime diagnostic snapshot for troubleshooting.
#[derive(Debug, Clone)]
pub struct CacheDiagnosticSnapshot {
    /// Effective cache configuration.
    pub config: CacheConfig,
    /// Current runtime counters.
    pub metrics: CacheMetricsSnapshot,
    /// Whether local backend is wired.
    pub local_backend_ready: bool,
    /// Whether remote backend is wired.
    pub remote_backend_ready: bool,
    /// Whether loader is configured.
    pub loader_configured: bool,
    /// Whether invalidation listener has been started.
    pub invalidation_listener_started: bool,
    /// Invalidation channel when broadcast listener is applicable.
    pub invalidation_channel: Option<String>,
}

/// Atomic metrics storage used by the cache instance.
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
    stale_fallback: AtomicU64,
    refresh_attempts: AtomicU64,
    refresh_success: AtomicU64,
    refresh_failures: AtomicU64,
    invalidation_publish: AtomicU64,
    invalidation_publish_failures: AtomicU64,
    invalidation_receive: AtomicU64,
    invalidation_receive_failures: AtomicU64,
}

impl CacheMetrics {
    /// Reads all atomic counters into a value snapshot.
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
            stale_fallback: self.stale_fallback.load(Ordering::Relaxed),
            refresh_attempts: self.refresh_attempts.load(Ordering::Relaxed),
            refresh_success: self.refresh_success.load(Ordering::Relaxed),
            refresh_failures: self.refresh_failures.load(Ordering::Relaxed),
            invalidation_publish: self.invalidation_publish.load(Ordering::Relaxed),
            invalidation_publish_failures: self
                .invalidation_publish_failures
                .load(Ordering::Relaxed),
            invalidation_receive: self.invalidation_receive.load(Ordering::Relaxed),
            invalidation_receive_failures: self
                .invalidation_receive_failures
                .load(Ordering::Relaxed),
        }
    }

    fn inc_local_hit(&self) {
        self.local_hit.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_local_miss(&self) {
        self.local_miss.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_remote_hit(&self) {
        self.remote_hit.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_remote_miss(&self) {
        self.remote_miss.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_load_total(&self) {
        self.load_total.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_load_success(&self) {
        self.load_success.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_load_timeout(&self) {
        self.load_timeout.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_load_error(&self) {
        self.load_error.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_stale_fallback(&self) {
        self.stale_fallback.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_refresh_attempt(&self) {
        self.refresh_attempts.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_refresh_success(&self) {
        self.refresh_success.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_refresh_failure(&self) {
        self.refresh_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_invalidation_publish(&self) {
        self.invalidation_publish.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_invalidation_publish_failure(&self) {
        self.invalidation_publish_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    fn inc_invalidation_receive(&self) {
        self.invalidation_receive.fetch_add(1, Ordering::Relaxed);
    }

    fn inc_invalidation_receive_failure(&self) {
        self.invalidation_receive_failures
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Pub/Sub payload used for cross-node local cache invalidation.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct InvalidationMessage {
    /// Logical cache area.
    area: String,
    /// Encoded keys to invalidate.
    keys: Vec<String>,
}

/// Multi-level cache with pluggable local/remote backends.
pub struct LevelCache<
    K,
    V,
    LD = NoopLoader,
    LB = local::MokaBackend<V>,
    RB = remote::RedisBackend<V>,
> where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
    LB: LocalBackend<V>,
    RB: RemoteBackend<V>,
{
    /// Runtime cache configuration.
    pub(crate) config: CacheConfig,
    /// Local in-memory backend (optional by mode).
    pub(crate) local: Option<LB>,
    /// Remote redis backend (optional by mode).
    pub(crate) remote: Option<RB>,
    /// Source-of-truth loader.
    pub(crate) loader: Option<LD>,
    /// Business-key encoder.
    pub(crate) key_converter: KeyConverter<K>,
    /// Singleflight group for miss deduplication.
    singleflight: Arc<SingleFlight<String, CacheResult<Option<V>>>>,
    /// Runtime metrics.
    metrics: Arc<CacheMetrics>,
    /// Marks whether invalidation listener has been started.
    invalidation_listener_started: AtomicBool,
    /// Join handle for invalidation listener background task.
    invalidation_listener_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl<K, V, LD, LB, RB> LevelCache<K, V, LD, LB, RB>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
    LB: LocalBackend<V>,
    RB: RemoteBackend<V>,
{
    /// Creates a cache instance from builder-wired components.
    pub(crate) fn new(
        config: CacheConfig,
        local: Option<LB>,
        remote: Option<RB>,
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
            invalidation_listener_started: AtomicBool::new(false),
            invalidation_listener_task: Mutex::new(None),
        }
    }

    /// Returns cache configuration.
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// Returns current metrics snapshot.
    pub fn metrics_snapshot(&self) -> CacheMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Returns a read-only runtime snapshot for diagnosis and debugging.
    pub fn diagnostic_snapshot(&self) -> CacheDiagnosticSnapshot {
        CacheDiagnosticSnapshot {
            config: self.config.clone(),
            metrics: self.metrics_snapshot(),
            local_backend_ready: self.local.is_some(),
            remote_backend_ready: self.remote.is_some(),
            loader_configured: self.loader.is_some(),
            invalidation_listener_started: self
                .invalidation_listener_started
                .load(Ordering::Acquire),
            invalidation_channel: self
                .should_start_invalidation_listener()
                .then(|| self.invalidation_channel()),
        }
    }

    /// Exports metrics as OpenTelemetry-friendly point list.
    pub fn otel_metric_points(&self) -> Vec<crate::observability::OtelMetricPoint> {
        crate::observability::to_otel_points(&self.config.area, &self.metrics_snapshot())
    }

    /// Reads one key from cache layers and falls back to loader on miss.
    #[instrument(name = "cache.get", skip_all, fields(area = %self.config.area))]
    pub async fn get(&self, key: &K, options: &ReadOptions) -> CacheResult<Option<V>> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let encoded_key = self.encoded_key(key);
        let key_hash = Self::hash_encoded_key(&encoded_key);

        let result = async {
            let stale_candidate = self.read_local_stale_value(&encoded_key).await?;

            if let Some(local_entry) = self.read_local_value(&encoded_key).await? {
                let expire_at = local_entry.expire_at;
                let value = Self::entry_to_value(local_entry);
                self.refresh_ahead_if_needed(key, &encoded_key, expire_at, options)
                    .await;
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

            match self.load_on_miss(key, &encoded_key).await {
                Ok(value) => Ok(value),
                Err(err) => self.stale_fallback_or_error(err, stale_candidate, options),
            }
        }
        .await;

        let result_tag = match &result {
            Ok(Some(_)) => "ok_some",
            Ok(None) => "ok_none",
            Err(_) => "error",
        };
        self.log_operation("get", key_hash, started, result_tag, result.as_ref().err());
        result
    }

    /// Reads multiple keys and returns a per-key optional value map.
    #[instrument(name = "cache.mget", skip_all, fields(area = %self.config.area, size = keys.len()))]
    pub async fn mget(
        &self,
        keys: &[K],
        options: &ReadOptions,
    ) -> CacheResult<HashMap<K, Option<V>>> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let encoded_pairs = keys
            .iter()
            .map(|key| (key.clone(), self.encoded_key(key)))
            .collect::<Vec<_>>();
        let encoded_keys = encoded_pairs
            .iter()
            .map(|(_, encoded_key)| encoded_key.clone())
            .collect::<Vec<_>>();
        let key_hash = Self::hash_encoded_keys(&encoded_keys);

        let result = async {
            let mut values = HashMap::with_capacity(keys.len());
            let mut misses = self.fill_from_local(encoded_pairs, &mut values).await?;
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
                    let value = self.load_on_miss(&key, &encoded_key).await?;
                    values.insert(key, value);
                }
                return Ok(values);
            }

            let loader = self
                .loader
                .as_ref()
                .ok_or_else(|| CacheError::InvalidConfig("loader is not configured".to_string()))?;

            let (request_keys, encoded_keys): (Vec<K>, Vec<String>) = misses.into_iter().unzip();

            let loaded_map = if let Some(timeout) = self.config.loader_timeout {
                match time::timeout(timeout, loader.mload(&request_keys)).await {
                    Ok(value) => value,
                    Err(_) => return Err(CacheError::Timeout("loader")),
                }
            } else {
                loader.mload(&request_keys).await
            }?;

            for (key, encoded_key) in request_keys.into_iter().zip(encoded_keys) {
                let loaded = loaded_map.get(&key).cloned().unwrap_or(None);
                self.write_by_mode(&encoded_key, loaded.clone()).await?;
                values.insert(key, loaded);
            }

            Ok(values)
        }
        .await;

        let result_tag = match &result {
            Ok(values) if values.is_empty() => "ok_empty",
            Ok(_) => "ok",
            Err(_) => "error",
        };
        self.log_operation("mget", key_hash, started, result_tag, result.as_ref().err());
        result
    }

    /// Writes one key to active cache layers.
    #[instrument(name = "cache.set", skip_all, fields(area = %self.config.area))]
    pub async fn set(&self, key: &K, value: Option<V>) -> CacheResult<()> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let encoded_key = self.encoded_key(key);
        let key_hash = Self::hash_encoded_key(&encoded_key);
        let result = self.write_by_mode(&encoded_key, value).await;
        self.log_operation("set", key_hash, started, "done", result.as_ref().err());
        result
    }

    /// Writes multiple key/value entries to cache layers.
    #[instrument(name = "cache.mset", skip_all, fields(area = %self.config.area, size = entries.len()))]
    pub async fn mset(&self, entries: HashMap<K, Option<V>>) -> CacheResult<()> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let mut encoded_entries = Vec::with_capacity(entries.len());
        for (key, value) in entries {
            encoded_entries.push((self.encoded_key(&key), value));
        }
        let encoded_keys = encoded_entries
            .iter()
            .map(|(encoded_key, _)| encoded_key.clone())
            .collect::<Vec<_>>();
        let key_hash = Self::hash_encoded_keys(&encoded_keys);

        let result = async {
            for (encoded_key, value) in encoded_entries {
                self.write_by_mode(&encoded_key, value).await?;
            }
            Ok(())
        }
        .await;

        self.log_operation("mset", key_hash, started, "done", result.as_ref().err());
        result
    }

    /// Deletes one key from all active cache layers.
    #[instrument(name = "cache.del", skip_all, fields(area = %self.config.area))]
    pub async fn del(&self, key: &K) -> CacheResult<()> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let encoded_key = self.encoded_key(key);
        let key_hash = Self::hash_encoded_key(&encoded_key);

        let result = async {
            self.delete_remote_if_needed(&encoded_key).await?;
            self.delete_local_if_needed(&encoded_key).await?;
            self.publish_invalidation_if_needed(vec![encoded_key])
                .await?;
            Ok(())
        }
        .await;

        self.log_operation("del", key_hash, started, "done", result.as_ref().err());
        result
    }

    /// Deletes multiple keys from all active cache layers.
    #[instrument(name = "cache.mdel", skip_all, fields(area = %self.config.area, size = keys.len()))]
    pub async fn mdel(&self, keys: &[K]) -> CacheResult<()> {
        self.ensure_invalidation_listener();
        let started = Instant::now();

        let encoded = keys
            .iter()
            .map(|key| self.encoded_key(key))
            .collect::<Vec<_>>();
        let key_hash = Self::hash_encoded_keys(&encoded);

        let result = async {
            if encoded.is_empty() {
                return Ok(());
            }

            self.batch_delete_remote_if_needed(&encoded).await?;
            self.batch_delete_local_if_needed(&encoded).await?;
            self.publish_invalidation_if_needed(encoded).await?;
            Ok(())
        }
        .await;

        self.log_operation("mdel", key_hash, started, "done", result.as_ref().err());
        result
    }

    /// Preloads keys through normal read path in configurable chunks.
    #[instrument(name = "cache.warmup", skip_all, fields(area = %self.config.area, size = keys.len()))]
    pub async fn warmup(&self, keys: &[K]) -> CacheResult<usize> {
        self.ensure_invalidation_listener();
        let started = Instant::now();
        let encoded_keys = keys
            .iter()
            .map(|key| self.encoded_key(key))
            .collect::<Vec<_>>();
        let key_hash = Self::hash_encoded_keys(&encoded_keys);

        let result = async {
            if !self.config.warmup_enabled {
                return Ok(0);
            }
            if keys.is_empty() {
                return Ok(0);
            }

            let options = ReadOptions {
                allow_stale: false,
                disable_load: false,
            };

            let mut loaded = 0usize;
            for chunk in keys.chunks(self.config.warmup_batch_size) {
                let values = self.mget(chunk, &options).await?;
                loaded += values.values().filter(|value| value.is_some()).count();
            }
            Ok(loaded)
        }
        .await;

        let result_tag = match &result {
            Ok(loaded) if *loaded == 0 => "ok_zero",
            Ok(_) => "ok_loaded",
            Err(_) => "error",
        };
        self.log_operation(
            "warmup",
            key_hash,
            started,
            result_tag,
            result.as_ref().err(),
        );
        result
    }

    /// Encodes business key into namespaced storage key.
    fn encoded_key(&self, key: &K) -> String {
        format!("{}:{}", self.config.area, (self.key_converter)(key))
    }

    /// Hashes one encoded key for logging without exposing raw key content.
    fn hash_encoded_key(encoded_key: &str) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        encoded_key.hash(&mut hasher);
        hasher.finish()
    }

    /// Hashes a batch of encoded keys for logging aggregation.
    fn hash_encoded_keys(encoded_keys: &[String]) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for encoded_key in encoded_keys {
            encoded_key.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Emits standardized operation log for governance and troubleshooting.
    fn log_operation(
        &self,
        op: &'static str,
        key_hash: u64,
        started: Instant,
        result: &'static str,
        error: Option<&CacheError>,
    ) {
        let latency_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        let error_kind = error.map(CacheError::kind).unwrap_or("none");

        if let Some(error) = error {
            tracing::warn!(
                target: "accelerator::ops",
                area = %self.config.area,
                op,
                key_hash,
                result,
                latency_ms,
                error_kind,
                error = %error,
                "cache operation completed",
            );
        } else {
            tracing::info!(
                target: "accelerator::ops",
                area = %self.config.area,
                op,
                key_hash,
                result,
                latency_ms,
                error_kind,
                "cache operation completed",
            );
        }
    }

    /// Returns whether local layer is active.
    fn mode_uses_local(&self) -> bool {
        matches!(self.config.mode, CacheMode::Local | CacheMode::Both)
    }

    /// Returns whether remote layer is active.
    fn mode_uses_remote(&self) -> bool {
        matches!(self.config.mode, CacheMode::Remote | CacheMode::Both)
    }

    /// Builds redis Pub/Sub channel name for invalidation.
    fn invalidation_channel(&self) -> String {
        format!("accelerator:invalidation:{}", self.config.area)
    }

    /// Checks whether invalidation subscriber should run.
    fn should_start_invalidation_listener(&self) -> bool {
        self.config.broadcast_invalidation
            && self.mode_uses_local()
            && self.mode_uses_remote()
            && self.local.is_some()
            && self.remote.is_some()
    }

    /// Lazily starts the invalidation listener exactly once.
    fn ensure_invalidation_listener(&self) {
        if !self.should_start_invalidation_listener() {
            return;
        }

        if self.invalidation_listener_started.load(Ordering::Acquire) {
            return;
        }

        let Some(local) = self.local.as_ref().cloned() else {
            return;
        };
        let Some(remote) = self.remote.as_ref().cloned() else {
            return;
        };

        if self
            .invalidation_listener_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }

        let channel = self.invalidation_channel();
        let metrics = self.metrics.clone();

        let handle = match tokio::runtime::Handle::try_current() {
            Ok(runtime) => runtime.spawn(async move {
                Self::run_invalidation_listener(local, remote, channel, metrics).await;
            }),
            Err(_) => {
                self.invalidation_listener_started
                    .store(false, Ordering::Release);
                return;
            }
        };

        let mut slot = self.invalidation_listener_task.lock().unwrap();
        *slot = Some(handle);
    }

    /// Runs resilient redis subscription loop and applies local invalidations.
    async fn run_invalidation_listener(
        local: LB,
        remote: RB,
        channel: String,
        metrics: Arc<CacheMetrics>,
    ) {
        // Keep retrying so temporary redis failures can self-heal.
        let mut retry_tick = time::interval(Duration::from_secs(1));
        retry_tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let mut subscriber = match remote.subscribe(&channel).await {
                Ok(subscriber) => subscriber,
                Err(_) => {
                    metrics.inc_invalidation_receive_failure();
                    retry_tick.tick().await;
                    continue;
                }
            };

            while let Some(payload_result) = subscriber.next_message().await {
                let Some(event) =
                    Self::decode_invalidation_message(payload_result, metrics.as_ref())
                else {
                    continue;
                };

                if event.keys.is_empty() {
                    continue;
                }

                if local.mdel(&event.keys).await.is_err() {
                    metrics.inc_invalidation_receive_failure();
                    continue;
                }
                metrics.inc_invalidation_receive();
            }

            retry_tick.tick().await;
        }
    }

    fn decode_invalidation_message(
        payload_result: CacheResult<String>,
        metrics: &CacheMetrics,
    ) -> Option<InvalidationMessage> {
        let Ok(payload) = payload_result else {
            metrics.inc_invalidation_receive_failure();
            return None;
        };

        let Ok(event) = serde_json::from_str::<InvalidationMessage>(&payload) else {
            metrics.inc_invalidation_receive_failure();
            return None;
        };

        Some(event)
    }

    /// Publishes delete invalidation events when configured.
    async fn publish_invalidation_if_needed(&self, keys: Vec<String>) -> CacheResult<()> {
        // Skip publish when feature off or no keys to broadcast.
        if !self.config.broadcast_invalidation || keys.is_empty() {
            return Ok(());
        }

        if !self.mode_uses_remote() {
            return Ok(());
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };

        let payload = serde_json::to_string(&InvalidationMessage {
            area: self.config.area.clone(),
            keys,
        })
        .map_err(|err| CacheError::Backend(format!("serialize invalidation failed: {err}")))?;

        self.metrics.inc_invalidation_publish();
        if let Err(err) = remote.publish(&self.invalidation_channel(), &payload).await {
            self.metrics.inc_invalidation_publish_failure();
            return Err(err);
        }

        Ok(())
    }

    /// Reads local entry without TTL filtering, used for stale fallback candidate.
    async fn read_local_stale_value(&self, encoded_key: &str) -> CacheResult<Option<Option<V>>> {
        if !self.mode_uses_local() {
            return Ok(None);
        }
        let Some(local) = self.local.as_ref() else {
            return Ok(None);
        };
        let Some(entry) = local.peek(encoded_key).await? else {
            return Ok(None);
        };
        Ok(Some(Self::entry_to_value(entry)))
    }

    /// Returns stale value on eligible errors, otherwise propagates original error.
    fn stale_fallback_or_error(
        &self,
        err: CacheError,
        stale_candidate: Option<Option<V>>,
        options: &ReadOptions,
    ) -> CacheResult<Option<V>> {
        if !(self.config.stale_on_error && options.allow_stale) {
            return Err(err);
        }

        let Some(value) = stale_candidate else {
            return Err(err);
        };

        self.metrics.inc_stale_fallback();
        Ok(value)
    }

    /// Triggers refresh-ahead when local value is close to expiration.
    async fn refresh_ahead_if_needed(
        &self,
        key: &K,
        encoded_key: &str,
        expire_at: Instant,
        options: &ReadOptions,
    ) {
        if !self.config.refresh_ahead || options.disable_load || self.loader.is_none() {
            return;
        }

        let remain = expire_at.saturating_duration_since(Instant::now());
        if remain > self.config.refresh_ahead_window {
            return;
        }

        self.metrics.inc_refresh_attempt();
        match self.load_on_miss(key, encoded_key).await {
            Ok(_) => {
                self.metrics.inc_refresh_success();
            }
            Err(_) => {
                self.metrics.inc_refresh_failure();
            }
        }
    }

    /// Reads one value from local backend and records hit/miss metrics.
    async fn read_local_value(&self, encoded_key: &str) -> CacheResult<Option<StoredEntry<V>>> {
        if !self.mode_uses_local() {
            return Ok(None);
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(None);
        };

        let entry = local.get(encoded_key).await?;
        if entry.is_some() {
            self.metrics.inc_local_hit();
        } else {
            self.metrics.inc_local_miss();
        }
        Ok(entry)
    }

    /// Reads one value from remote and backfills local layer when enabled.
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
            self.metrics.inc_remote_miss();
            return Ok(None);
        };

        self.metrics.inc_remote_hit();
        let value = Self::entry_to_value(entry);
        self.backfill_local_if_needed(encoded_key, value.clone())
            .await?;
        Ok(Some(value))
    }

    /// Backfills local cache only when local mode is active.
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

    /// Deletes remote key only when remote mode is active.
    async fn delete_remote_if_needed(&self, encoded_key: &str) -> CacheResult<()> {
        if !self.mode_uses_remote() {
            return Ok(());
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };

        remote.del(encoded_key).await
    }

    /// Deletes local key only when local mode is active.
    async fn delete_local_if_needed(&self, encoded_key: &str) -> CacheResult<()> {
        if !self.mode_uses_local() {
            return Ok(());
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(());
        };

        local.del(encoded_key).await
    }

    /// Batch deletes remote keys only when remote mode is active.
    async fn batch_delete_remote_if_needed(&self, encoded_keys: &[String]) -> CacheResult<()> {
        if !self.mode_uses_remote() {
            return Ok(());
        }

        let Some(remote) = self.remote.as_ref() else {
            return Ok(());
        };

        remote.mdel(encoded_keys).await
    }

    /// Batch deletes local keys only when local mode is active.
    async fn batch_delete_local_if_needed(&self, encoded_keys: &[String]) -> CacheResult<()> {
        if !self.mode_uses_local() {
            return Ok(());
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(());
        };

        local.mdel(encoded_keys).await
    }

    /// Fills result map from local layer and returns remaining misses.
    async fn fill_from_local(
        &self,
        encoded_pairs: Vec<(K, String)>,
        values: &mut HashMap<K, Option<V>>,
    ) -> CacheResult<Vec<(K, String)>> {
        if !self.mode_uses_local() {
            return Ok(encoded_pairs);
        }

        let Some(local) = self.local.as_ref() else {
            return Ok(encoded_pairs);
        };
        let keys = encoded_pairs
            .iter()
            .map(|(_, encoded)| encoded.clone())
            .collect::<Vec<_>>();
        let local_values = local.mget(&keys).await?;

        let mut misses = Vec::new();
        for (key, encoded_key) in encoded_pairs {
            let Some(entry) = local_values.get(&encoded_key).cloned().flatten() else {
                self.metrics.inc_local_miss();
                misses.push((key, encoded_key));
                continue;
            };

            self.metrics.inc_local_hit();
            let value = Self::entry_to_value(entry);
            values.insert(key, value);
        }

        Ok(misses)
    }

    /// Fills misses from remote layer and backfills local for remote hits.
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
            let Some(entry) = remote_values.get(&encoded_key).cloned().flatten() else {
                self.metrics.inc_remote_miss();
                remained.push((key, encoded_key));
                continue;
            };

            self.metrics.inc_remote_hit();
            let value = Self::entry_to_value(entry);
            values.insert(key, value.clone());

            if !can_backfill_local {
                continue;
            }

            let backfill_entry = Self::to_entry(
                &encoded_key,
                value,
                self.config.local_ttl,
                self.config.null_ttl,
                self.config.ttl_jitter_ratio,
            );
            backfill_entries.insert(encoded_key, backfill_entry);
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

    /// Writes one entry into local cache.
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

    /// Loads value on miss, optionally through singleflight deduplication.
    #[instrument(name = "cache.load", skip_all, fields(area = %self.config.area))]
    async fn load_on_miss(&self, key: &K, encoded_key: &str) -> CacheResult<Option<V>> {
        if self.loader.is_none() {
            return Err(CacheError::InvalidConfig(
                "loader is not configured".to_string(),
            ));
        }

        if !self.config.penetration_protect {
            return self.load_and_write(key, encoded_key).await;
        }

        self.singleflight
            .work(encoded_key.to_string(), || async {
                self.load_and_write(key, encoded_key).await
            })
            .await
    }

    /// Executes loader with timeout/metrics and writes result back to cache.
    async fn load_and_write(&self, key: &K, encoded_key: &str) -> CacheResult<Option<V>> {
        let loader = self
            .loader
            .as_ref()
            .ok_or_else(|| CacheError::InvalidConfig("loader is not configured".to_string()))?;

        self.metrics.inc_load_total();
        let load_future = loader.load(key);
        let loaded = if let Some(timeout) = self.config.loader_timeout {
            match time::timeout(timeout, load_future).await {
                Ok(value) => value,
                Err(_) => {
                    self.metrics.inc_load_timeout();
                    return Err(CacheError::Timeout("loader"));
                }
            }
        } else {
            load_future.await
        };

        let loaded = match loaded {
            Ok(value) => value,
            Err(err) => {
                self.metrics.inc_load_error();
                return Err(err);
            }
        };

        self.metrics.inc_load_success();
        self.write_by_mode(encoded_key, loaded.clone()).await?;
        Ok(loaded)
    }

    /// Writes one key to cache backends according to configured mode.
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

    /// Writes one key into local backend with ttl/null policy.
    async fn write_local(
        local: &Option<LB>,
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

    /// Writes one key into remote backend with ttl/null policy.
    async fn write_remote(
        remote: &Option<RB>,
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

    /// Converts stored entry payload into user-facing optional value.
    fn entry_to_value(entry: StoredEntry<V>) -> Option<V> {
        match entry.value {
            StoredValue::Value(value) => Some(value),
            StoredValue::Null => None,
        }
    }

    /// Builds a new entry using cache-level ttl and jitter policy.
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

    /// Applies deterministic key-based ttl jitter to reduce avalanche risk.
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

    /// Converts optional value into a stored entry with proper ttl.
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

impl<K, V, LD, LB, RB> Drop for LevelCache<K, V, LD, LB, RB>
where
    K: Clone + Eq + Hash + ToString + Send + Sync + 'static,
    V: Clone + Serialize + DeserializeOwned + Send + Sync + 'static,
    LD: MLoader<K, V> + Send + Sync + 'static,
    LB: LocalBackend<V>,
    RB: RemoteBackend<V>,
{
    /// Aborts background invalidation task when cache instance is dropped.
    fn drop(&mut self) {
        if let Some(task) = self.invalidation_listener_task.get_mut().unwrap().take() {
            task.abort();
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

        LevelCacheBuilder::<u64, String>::new()
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

    #[tokio::test]
    async fn diagnostic_snapshot_contains_effective_config_and_flags() {
        let cache = test_cache_builder()
            .area("diag-area")
            .broadcast_invalidation(true)
            .loader_fn(|key: u64| async move { Ok(Some(format!("v-{key}"))) })
            .build()
            .unwrap();

        let _ = cache.get(&100, &ReadOptions::default()).await.unwrap();
        let snapshot = cache.diagnostic_snapshot();

        assert_eq!(snapshot.config.area, "diag-area");
        assert!(snapshot.loader_configured);
        assert!(snapshot.local_backend_ready);
        assert!(!snapshot.remote_backend_ready);
        assert!(!snapshot.invalidation_listener_started);
        assert!(snapshot.invalidation_channel.is_none());
        assert_eq!(snapshot.metrics.load_success, 1);
    }

    #[tokio::test]
    async fn metrics_export_helpers_include_area_label() {
        let cache = test_cache_builder().area("metric-area").build().unwrap();
        cache.set(&9, Some("v9".to_string())).await.unwrap();
        let _ = cache.get(&9, &ReadOptions::default()).await.unwrap();

        let otel_points = cache.otel_metric_points();
        assert_eq!(otel_points.len(), 16);
        assert!(
            otel_points
                .iter()
                .all(|point| { point.attributes == vec![("area", "metric-area".to_string())] })
        );
    }

    #[tokio::test]
    async fn warm_up_preloads_values_with_loader() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .warmup_enabled(true)
            .warmup_batch_size(2)
            .loader_fn(move |key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(format!("warm-{key}")))
                }
            })
            .build()
            .unwrap();

        let loaded = cache.warmup(&[11, 12, 13]).await.unwrap();
        assert_eq!(loaded, 3);
        assert_eq!(loads.load(Ordering::SeqCst), 3);

        let values = cache
            .mget(&[11, 12, 13], &ReadOptions::default())
            .await
            .unwrap();
        assert_eq!(
            values.get(&11).cloned().flatten(),
            Some("warm-11".to_string())
        );
        assert_eq!(
            values.get(&12).cloned().flatten(),
            Some("warm-12".to_string())
        );
        assert_eq!(
            values.get(&13).cloned().flatten(),
            Some("warm-13".to_string())
        );
    }

    #[tokio::test]
    async fn warmup_is_noop_when_disabled() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .warmup_enabled(false)
            .loader_fn(move |key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    Ok(Some(format!("warm-{key}")))
                }
            })
            .build()
            .unwrap();

        let loaded = cache.warmup(&[1, 2, 3]).await.unwrap();
        assert_eq!(loaded, 0);
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stale_fallback_returns_last_local_value() {
        let cache = test_cache_builder()
            .local_ttl(Duration::from_millis(30))
            .stale_on_error(true)
            .loader_fn(|_key: u64| async move {
                Err(crate::error::CacheError::Loader("db down".to_string()))
            })
            .build()
            .unwrap();

        cache
            .set(&77, Some("stale-value".to_string()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;

        let value = cache
            .get(
                &77,
                &ReadOptions {
                    allow_stale: true,
                    disable_load: false,
                },
            )
            .await
            .unwrap();
        assert_eq!(value, Some("stale-value".to_string()));

        let metrics = cache.metrics_snapshot();
        assert_eq!(metrics.stale_fallback, 1);
    }

    #[tokio::test]
    async fn refresh_ahead_updates_cached_value_before_expire() {
        let loads = Arc::new(AtomicUsize::new(0));
        let loads_for_loader = loads.clone();

        let cache = test_cache_builder()
            .local_ttl(Duration::from_millis(120))
            .refresh_ahead(true)
            .refresh_ahead_window(Duration::from_millis(30))
            .loader_fn(move |key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    let call = loads_for_loader.fetch_add(1, Ordering::SeqCst) + 1;
                    Ok(Some(format!("refreshed-{key}-{call}")))
                }
            })
            .build()
            .unwrap();

        cache.set(&9, Some("seed".to_string())).await.unwrap();
        tokio::time::sleep(Duration::from_millis(95)).await;

        let first = cache.get(&9, &ReadOptions::default()).await.unwrap();
        assert_eq!(first, Some("seed".to_string()));

        let second = cache.get(&9, &ReadOptions::default()).await.unwrap();
        assert_eq!(second, Some("refreshed-9-1".to_string()));
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        let metrics = cache.metrics_snapshot();
        assert_eq!(metrics.refresh_attempts, 1);
        assert_eq!(metrics.refresh_success, 1);
        assert_eq!(metrics.refresh_failures, 0);
    }
}
