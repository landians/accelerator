use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    Local,
    Remote,
    Both,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadValueMode {
    OwnedClone,
    SharedArc,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub area: String,
    pub mode: CacheMode,
    pub local_ttl: Duration,
    pub remote_ttl: Duration,
    pub null_ttl: Duration,
    pub ttl_jitter_ratio: Option<f64>,
    pub cache_null_value: bool,
    pub penetration_protect: bool,
    pub loader_timeout: Option<Duration>,
    pub copy_on_read: bool,
    pub copy_on_write: bool,
    pub read_value_mode: ReadValueMode,
}

impl Default for CacheConfig {
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
            copy_on_read: true,
            copy_on_write: true,
            read_value_mode: ReadValueMode::OwnedClone,
        }
    }
}
