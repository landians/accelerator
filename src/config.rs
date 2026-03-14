use std::time::Duration;

/// Cache topology mode for fixed backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// Use only in-memory (Moka) layer.
    Local,
    /// Use only remote (Redis) layer.
    Remote,
    /// Use both in-memory + remote layers.
    Both,
}

/// Read-time value ownership strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadValueMode {
    /// Return cloned owned value.
    OwnedClone,
    /// Reserved for future Arc-based shared reads.
    SharedArc,
}

/// Runtime configuration for `LevelCache`.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Logical namespace used in encoded keys and invalidation channel.
    pub area: String,
    /// Which cache layers are enabled.
    pub mode: CacheMode,
    /// TTL for local entries that contain real value.
    pub local_ttl: Duration,
    /// TTL for remote entries that contain real value.
    pub remote_ttl: Duration,
    /// TTL used when caching `None`.
    pub null_ttl: Duration,
    /// Optional jitter ratio in `[0.0, 1.0]` to spread expiration.
    pub ttl_jitter_ratio: Option<f64>,
    /// Whether `None` should be cached as negative-cache marker.
    pub cache_null_value: bool,
    /// Whether to enable singleflight dedup on single-key miss load paths.
    ///
    /// Note: batched `mget` miss handling uses `MLoader::mload` directly.
    pub penetration_protect: bool,
    /// Optional timeout for loader `load/mload`.
    pub loader_timeout: Option<Duration>,
    /// If `false`, `warmup` becomes a no-op.
    pub warmup_enabled: bool,
    /// Keys per chunk when warmup is enabled.
    pub warmup_batch_size: usize,
    /// Enables refresh-ahead on reads near expiration.
    pub refresh_ahead: bool,
    /// Trigger refresh-ahead when remaining TTL <= this window.
    pub refresh_ahead_window: Duration,
    /// Allows stale fallback on loader error when request allows stale.
    pub stale_on_error: bool,
    /// Publishes invalidation events via Redis Pub/Sub on delete paths.
    pub broadcast_invalidation: bool,
    /// Read value mode strategy.
    pub read_value_mode: ReadValueMode,
}

impl Default for CacheConfig {
    /// Returns a conservative default config suitable for MVP usage.
    fn default() -> Self {
        Self {
            area: "default".to_string(),
            mode: CacheMode::Both,
            local_ttl: Duration::from_secs(60),
            remote_ttl: Duration::from_secs(300),
            null_ttl: Duration::from_secs(30),
            ttl_jitter_ratio: Some(0.1),
            cache_null_value: true,
            penetration_protect: true,
            loader_timeout: Some(Duration::from_millis(300)),
            warmup_enabled: false,
            warmup_batch_size: 256,
            refresh_ahead: false,
            refresh_ahead_window: Duration::from_secs(3),
            stale_on_error: false,
            broadcast_invalidation: false,
            read_value_mode: ReadValueMode::OwnedClone,
        }
    }
}
