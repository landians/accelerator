use std::time::Instant;

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
