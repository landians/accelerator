extern crate self as accelerator;

/// Shared backend value types.
pub mod backend;
/// Cache builder API.
pub mod builder;
/// Core cache implementation.
pub mod cache;
/// Configuration model.
pub mod config;
/// Error and result definitions.
pub mod error;
/// Loader abstractions and adapters.
pub mod loader;
/// Local cache backend (moka).
pub mod local;
/// Metrics/logging exporter helpers.
pub mod observability;
/// Remote cache backend (redis).
pub mod remote;
/// Optional OTLP telemetry bootstrap helpers (feature: `otlp`).
#[cfg(feature = "otlp")]
pub mod telemetry;
/// Procedural macros re-export.
pub mod macros {
    pub use macros_impl::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};
}

/// Re-export of backend storage value types.
pub use backend::{StoredEntry, StoredValue};
/// Re-export of core cache API.
pub use cache::{CacheDiagnosticSnapshot, CacheMetricsSnapshot, LevelCache, ReadOptions};
/// Re-export of key configuration enums.
pub use config::{CacheMode, ReadValueMode};
/// Re-export of error and result types.
pub use error::{CacheError, CacheResult};
/// Re-export of loader traits and helpers.
pub use loader::{FnLoader, Loader, MLoader, NoopLoader};
/// Re-export of observability helpers.
pub use observability::{
    MetricPoint, OtelMetricPoint, metric_points, render_prometheus, to_otel_points,
};
/// Re-export of optional OTLP telemetry helpers.
#[cfg(feature = "otlp")]
pub use telemetry::{OtlpTelemetryBuilder, OtlpTransport, TelemetryGuard, TelemetryInitError};
/// Re-export of tracing for macro-generated logging.
pub use tracing;
