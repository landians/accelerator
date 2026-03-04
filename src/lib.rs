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
/// Remote cache backend (redis).
pub mod remote;

/// Re-export of backend storage value types.
pub use backend::{StoredEntry, StoredValue};
/// Re-export of core cache API.
pub use cache::{CacheMetricsSnapshot, LevelCache, ReadOptions};
/// Re-export of key configuration enums.
pub use config::{CacheMode, ReadValueMode};
/// Re-export of error and result types.
pub use error::{CacheError, CacheResult};
/// Re-export of loader traits and helpers.
pub use loader::{FnLoader, Loader, MLoader, NoopLoader};
