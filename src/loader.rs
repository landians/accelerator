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
    use std::collections::HashMap;
    use std::hash::Hash;
    use std::sync::{Arc, Mutex};

    use crate::error::{CacheError, CacheResult};
    use crate::loader::{FnLoader, Loader, MLoader, NoopLoader};

    fn map_keys_to_optional_values<K, V>(keys: &[K], found: &HashMap<K, V>) -> HashMap<K, Option<V>>
    where
        K: Clone + Eq + Hash,
        V: Clone,
    {
        let mut result = HashMap::with_capacity(keys.len());
        for key in keys {
            result.insert(key.clone(), found.get(key).cloned());
        }
        result
    }

    #[derive(Clone)]
    struct FakeUserRepo {
        rows: Arc<HashMap<u64, String>>,
        single_queries: Arc<Mutex<Vec<u64>>>,
        batched_queries: Arc<Mutex<Vec<Vec<u64>>>>,
    }

    impl FakeUserRepo {
        fn new(rows: HashMap<u64, String>) -> Self {
            Self {
                rows: Arc::new(rows),
                single_queries: Arc::new(Mutex::new(Vec::new())),
                batched_queries: Arc::new(Mutex::new(Vec::new())),
            }
        }

        async fn find_by_id(&self, id: u64) -> Result<Option<String>, &'static str> {
            self.single_queries.lock().unwrap().push(id);
            Ok(self.rows.get(&id).cloned())
        }

        async fn find_by_ids(&self, ids: &[u64]) -> Result<HashMap<u64, String>, &'static str> {
            self.batched_queries.lock().unwrap().push(ids.to_vec());

            let mut found = HashMap::with_capacity(ids.len());
            for id in ids {
                if let Some(value) = self.rows.get(id) {
                    found.insert(*id, value.clone());
                }
            }
            Ok(found)
        }

        fn recorded_single_queries(&self) -> Vec<u64> {
            self.single_queries.lock().unwrap().clone()
        }

        fn recorded_queries(&self) -> Vec<Vec<u64>> {
            self.batched_queries.lock().unwrap().clone()
        }
    }

    #[derive(Clone)]
    struct FakeUserRepoLoader {
        repo: Arc<FakeUserRepo>,
    }

    impl FakeUserRepoLoader {
        fn new(repo: Arc<FakeUserRepo>) -> Self {
            Self { repo }
        }
    }

    impl Loader<u64, String> for FakeUserRepoLoader {
        async fn load(&self, key: &u64) -> CacheResult<Option<String>> {
            self.repo
                .find_by_id(*key)
                .await
                .map_err(|err| CacheError::Loader(format!("find_by_id failed: {err}")))
        }
    }

    impl MLoader<u64, String> for FakeUserRepoLoader {
        async fn mload(&self, keys: &[u64]) -> CacheResult<HashMap<u64, Option<String>>> {
            let found = self
                .repo
                .find_by_ids(keys)
                .await
                .map_err(|err| CacheError::Loader(format!("find_by_ids failed: {err}")))?;
            Ok(map_keys_to_optional_values(keys, &found))
        }
    }

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

    #[tokio::test]
    async fn repo_style_loader_uses_repo_single_query_method() {
        let repo = Arc::new(FakeUserRepo::new(HashMap::from([(
            7_u64,
            "user-7".to_string(),
        )])));
        let loader = FakeUserRepoLoader::new(repo.clone());

        let value = Loader::<u64, String>::load(&loader, &7).await.unwrap();
        let miss = Loader::<u64, String>::load(&loader, &8).await.unwrap();

        assert_eq!(value, Some("user-7".to_string()));
        assert_eq!(miss, None);
        assert_eq!(repo.recorded_single_queries(), vec![7, 8]);
    }

    #[tokio::test]
    async fn repo_style_mloader_covers_all_requested_keys() {
        let rows = HashMap::from([(7_u64, "user-7".to_string()), (9_u64, "user-9".to_string())]);
        let repo = Arc::new(FakeUserRepo::new(rows));
        let loader = FakeUserRepoLoader::new(repo.clone());

        let values = MLoader::<u64, String>::mload(&loader, &[7, 8, 7, 9])
            .await
            .unwrap();

        assert_eq!(values.len(), 3);
        assert_eq!(
            values.get(&7).cloned().flatten(),
            Some("user-7".to_string())
        );
        assert_eq!(values.get(&8).cloned().flatten(), None);
        assert_eq!(
            values.get(&9).cloned().flatten(),
            Some("user-9".to_string())
        );

        let queries = repo.recorded_queries();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0], vec![7, 8, 7, 9]);
    }
}
