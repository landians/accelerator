use std::collections::HashMap;
use std::future::Future;
use std::marker::PhantomData;

use crate::error::CacheResult;

#[allow(async_fn_in_trait)]
pub trait Loader<K, V>: Send + Sync
where
    K: Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn load(&self, key: &K) -> CacheResult<Option<V>>;
}

#[allow(async_fn_in_trait)]
pub trait MLoader<K, V>: Loader<K, V>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn mload(&self, keys: &[K]) -> CacheResult<HashMap<K, Option<V>>>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NoopLoader;

impl<K, V> Loader<K, V> for NoopLoader
where
    K: Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn load(&self, _key: &K) -> CacheResult<Option<V>> {
        Ok(None)
    }
}

impl<K, V> MLoader<K, V> for NoopLoader
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn mload(&self, keys: &[K]) -> CacheResult<HashMap<K, Option<V>>> {
        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(key.clone(), None);
        }
        Ok(values)
    }
}

pub struct FnLoader<K, V, F, Fut>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: Fn(K) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CacheResult<Option<V>>> + Send + 'static,
{
    load_fn: F,
    _marker: PhantomData<(K, V)>,
}

impl<K, V, F, Fut> FnLoader<K, V, F, Fut>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: Fn(K) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CacheResult<Option<V>>> + Send + 'static,
{
    pub fn new(load_fn: F) -> Self {
        Self {
            load_fn,
            _marker: PhantomData,
        }
    }
}

impl<K, V, F, Fut> Loader<K, V> for FnLoader<K, V, F, Fut>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: Fn(K) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CacheResult<Option<V>>> + Send + 'static,
{
    async fn load(&self, key: &K) -> CacheResult<Option<V>> {
        (self.load_fn)(key.clone()).await
    }
}

impl<K, V, F, Fut> MLoader<K, V> for FnLoader<K, V, F, Fut>
where
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    F: Fn(K) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = CacheResult<Option<V>>> + Send + 'static,
{
    async fn mload(&self, keys: &[K]) -> CacheResult<HashMap<K, Option<V>>> {
        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(key.clone(), (self.load_fn)(key.clone()).await?);
        }
        Ok(values)
    }
}

#[cfg(test)]
mod tests {
    use crate::loader::{FnLoader, Loader, MLoader, NoopLoader};

    #[tokio::test]
    async fn noop_loader_returns_none_for_all_keys() {
        let loader = NoopLoader;
        let values = MLoader::<u64, String>::mload(&loader, &[1, 2, 3])
            .await
            .unwrap();

        assert_eq!(values.len(), 3);
        assert!(values.values().all(|value| value.is_none()));
    }

    #[tokio::test]
    async fn fn_loader_supports_load_and_mload() {
        let loader = FnLoader::new(|key: u64| async move { Ok(Some(format!("value-{key}"))) });

        let one = Loader::<u64, String>::load(&loader, &7).await.unwrap();
        assert_eq!(one, Some("value-7".to_string()));

        let values = MLoader::<u64, String>::mload(&loader, &[7, 8])
            .await
            .unwrap();
        assert_eq!(
            values.get(&7).cloned().flatten(),
            Some("value-7".to_string())
        );
        assert_eq!(
            values.get(&8).cloned().flatten(),
            Some("value-8".to_string())
        );
    }
}
