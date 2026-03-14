use std::collections::HashMap;
use std::future::Future;
use std::time::Instant;

use crate::error::CacheResult;

/// Value representation stored in cache backends.
///
/// `Null` is used for negative caching (cache penetration protection), so
/// cache layers can distinguish "not found but cached" from "key missing".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredValue<V> {
    /// Real business value.
    Value(V),
    /// Explicitly cached "not found".
    Null,
}

/// A backend entry that bundles payload and its absolute expiration time.
#[derive(Clone, Debug)]
pub struct StoredEntry<V> {
    /// Payload, including negative-cache marker.
    pub value: StoredValue<V>,
    /// Absolute expiration timestamp used by in-memory checks.
    pub expire_at: Instant,
}

impl<V> StoredEntry<V> {
    /// Returns `true` when the entry has expired.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expire_at
    }
}

/// Local cache backend abstraction.
pub trait LocalBackend<V>: Clone + Send + Sync + 'static
where
    V: Clone + Send + Sync + 'static,
{
    /// Reads one key and applies backend-specific expiration handling.
    fn get(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;

    /// Reads one key without expiration filtering (for stale fallback).
    fn peek(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;

    /// Reads multiple keys and returns key-indexed optional entries.
    fn mget(
        &self,
        keys: &[String],
    ) -> impl Future<Output = CacheResult<HashMap<String, Option<StoredEntry<V>>>>> + Send;

    /// Writes one key/value entry.
    fn set(&self, key: &str, entry: StoredEntry<V>)
    -> impl Future<Output = CacheResult<()>> + Send;

    /// Writes multiple entries.
    fn mset(
        &self,
        entries: HashMap<String, StoredEntry<V>>,
    ) -> impl Future<Output = CacheResult<()>> + Send;

    /// Deletes one key.
    fn del(&self, key: &str) -> impl Future<Output = CacheResult<()>> + Send;

    /// Deletes multiple keys.
    fn mdel(&self, keys: &[String]) -> impl Future<Output = CacheResult<()>> + Send;
}

/// Streaming subscriber for remote invalidation messages.
pub trait InvalidationSubscriber: Send {
    /// Returns next payload message, or `None` when stream ends.
    fn next_message(&mut self) -> impl Future<Output = Option<CacheResult<String>>> + Send;
}

/// Remote cache backend abstraction with invalidation pub/sub support.
pub trait RemoteBackend<V>: Clone + Send + Sync + 'static
where
    V: Clone + Send + Sync + 'static,
{
    /// Backend-specific subscription type for invalidation payloads.
    type Subscriber: InvalidationSubscriber + Send + 'static;

    /// Reads one key.
    fn get(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;

    /// Reads multiple keys and returns key-indexed optional entries.
    fn mget(
        &self,
        keys: &[String],
    ) -> impl Future<Output = CacheResult<HashMap<String, Option<StoredEntry<V>>>>> + Send;

    /// Writes one key/value entry.
    fn set(&self, key: &str, entry: StoredEntry<V>)
    -> impl Future<Output = CacheResult<()>> + Send;

    /// Writes multiple entries.
    fn mset(
        &self,
        entries: HashMap<String, StoredEntry<V>>,
    ) -> impl Future<Output = CacheResult<()>> + Send;

    /// Deletes one key.
    fn del(&self, key: &str) -> impl Future<Output = CacheResult<()>> + Send;

    /// Deletes multiple keys.
    fn mdel(&self, keys: &[String]) -> impl Future<Output = CacheResult<()>> + Send;

    /// Publishes an invalidation payload to a topic/channel.
    fn publish(&self, channel: &str, payload: &str)
    -> impl Future<Output = CacheResult<()>> + Send;

    /// Opens an invalidation subscription stream.
    fn subscribe(
        &self,
        channel: &str,
    ) -> impl Future<Output = CacheResult<Self::Subscriber>> + Send;
}
