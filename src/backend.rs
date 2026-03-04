use std::time::Instant;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredValue<V> {
    Value(V),
    Null,
}

#[derive(Clone, Debug)]
pub struct StoredEntry<V> {
    pub value: StoredValue<V>,
    pub expire_at: Instant,
}

impl<V> StoredEntry<V> {
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expire_at
    }
}
