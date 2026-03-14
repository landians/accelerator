use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use accelerator::cache::ReadOptions;
use accelerator::macros::{cache_evict_batch, cacheable_batch};
use accelerator::{CacheError, CacheResult};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct User {
    pub(crate) id: u64,
    pub(crate) name: String,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum MacroTestError {
    #[error("cache error: {0}")]
    Cache(#[from] CacheError),
    #[error("business error: {0}")]
    Biz(&'static str),
}

pub(crate) type MacroTestResult<T> = Result<T, MacroTestError>;

/// A batch-capable test cache with configurable failures.
#[derive(Clone)]
pub(crate) struct FailingBatchCache<V>
where
    V: Clone + Send + 'static,
{
    store: Arc<Mutex<HashMap<u64, Option<V>>>>,
    fail_mget: Arc<AtomicBool>,
    fail_mset: Arc<AtomicBool>,
    fail_mdel: Arc<AtomicBool>,
    mget_calls: Arc<AtomicUsize>,
    mset_calls: Arc<AtomicUsize>,
    mdel_calls: Arc<AtomicUsize>,
    last_mset_sizes: Arc<Mutex<Vec<usize>>>,
    last_mdel_keys: Arc<Mutex<Vec<Vec<u64>>>>,
}

impl<V> Default for FailingBatchCache<V>
where
    V: Clone + Send + 'static,
{
    fn default() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
            fail_mget: Arc::new(AtomicBool::new(false)),
            fail_mset: Arc::new(AtomicBool::new(false)),
            fail_mdel: Arc::new(AtomicBool::new(false)),
            mget_calls: Arc::new(AtomicUsize::new(0)),
            mset_calls: Arc::new(AtomicUsize::new(0)),
            mdel_calls: Arc::new(AtomicUsize::new(0)),
            last_mset_sizes: Arc::new(Mutex::new(Vec::new())),
            last_mdel_keys: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl<V> FailingBatchCache<V>
where
    V: Clone + Send + 'static,
{
    pub(crate) fn seed(&self, key: u64, value: Option<V>) {
        self.store.lock().unwrap().insert(key, value);
    }

    pub(crate) fn read(&self, key: u64) -> Option<V> {
        self.store
            .lock()
            .unwrap()
            .get(&key)
            .cloned()
            .unwrap_or(None)
    }

    pub(crate) fn set_fail_mget(&self, enabled: bool) {
        self.fail_mget.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_mset(&self, enabled: bool) {
        self.fail_mset.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_mdel(&self, enabled: bool) {
        self.fail_mdel.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn mget_calls(&self) -> usize {
        self.mget_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn mset_calls(&self) -> usize {
        self.mset_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn mdel_calls(&self) -> usize {
        self.mdel_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn last_mset_sizes(&self) -> Vec<usize> {
        self.last_mset_sizes.lock().unwrap().clone()
    }

    pub(crate) fn last_mdel_keys(&self) -> Vec<Vec<u64>> {
        self.last_mdel_keys.lock().unwrap().clone()
    }

    pub(crate) async fn mget(
        &self,
        keys: &[u64],
        _opts: &ReadOptions,
    ) -> CacheResult<HashMap<u64, Option<V>>> {
        self.mget_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_mget.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock mget failed".to_string()));
        }

        let store = self.store.lock().unwrap();
        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(*key, store.get(key).cloned().unwrap_or(None));
        }
        Ok(values)
    }

    pub(crate) async fn mset(&self, entries: HashMap<u64, Option<V>>) -> CacheResult<()> {
        self.mset_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_mset.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock mset failed".to_string()));
        }

        self.last_mset_sizes.lock().unwrap().push(entries.len());
        let mut store = self.store.lock().unwrap();
        for (key, value) in entries {
            store.insert(key, value);
        }
        Ok(())
    }

    pub(crate) async fn mdel(&self, keys: &[u64]) -> CacheResult<()> {
        self.mdel_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_mdel.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock mdel failed".to_string()));
        }

        self.last_mdel_keys.lock().unwrap().push(keys.to_vec());
        let mut store = self.store.lock().unwrap();
        for key in keys {
            store.remove(key);
        }
        Ok(())
    }
}

pub(crate) struct CacheableBatchHarness {
    pub(crate) cache: FailingBatchCache<User>,
    repo: Arc<HashMap<u64, User>>,
    load_inputs: Arc<Mutex<Vec<Vec<u64>>>>,
}

impl CacheableBatchHarness {
    pub(crate) fn new() -> Self {
        let repo = HashMap::from([
            (
                1,
                User {
                    id: 1,
                    name: "u1".to_string(),
                },
            ),
            (
                2,
                User {
                    id: 2,
                    name: "u2".to_string(),
                },
            ),
            (
                3,
                User {
                    id: 3,
                    name: "u3".to_string(),
                },
            ),
        ]);

        Self {
            cache: FailingBatchCache::default(),
            repo: Arc::new(repo),
            load_inputs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    #[cacheable_batch(cache = self.cache, keys = user_ids)]
    pub(crate) async fn load_users(
        &self,
        user_ids: Vec<u64>,
    ) -> CacheResult<HashMap<u64, Option<User>>> {
        self.load_inputs.lock().unwrap().push(user_ids.clone());
        Ok(self.build_batch_map(&user_ids))
    }

    #[cacheable_batch(cache = self.cache, keys = user_ids, on_cache_error = "propagate")]
    pub(crate) async fn load_users_propagate(
        &self,
        user_ids: Vec<u64>,
    ) -> MacroTestResult<HashMap<u64, Option<User>>> {
        self.load_inputs.lock().unwrap().push(user_ids.clone());
        Ok(self.build_batch_map(&user_ids))
    }

    #[cacheable_batch(cache = self.cache, keys = user_ids)]
    pub(crate) async fn load_users_fail(
        &self,
        user_ids: Vec<u64>,
    ) -> MacroTestResult<HashMap<u64, Option<User>>> {
        self.load_inputs.lock().unwrap().push(user_ids.clone());
        Err::<HashMap<u64, Option<User>>, MacroTestError>(MacroTestError::Biz("batch-load-failed"))
    }

    pub(crate) fn load_inputs(&self) -> Vec<Vec<u64>> {
        self.load_inputs.lock().unwrap().clone()
    }

    fn build_batch_map(&self, keys: &[u64]) -> HashMap<u64, Option<User>> {
        let mut map = HashMap::with_capacity(keys.len());
        for key in keys {
            map.insert(*key, self.repo.get(key).cloned());
        }
        map
    }
}

pub(crate) struct CacheEvictBatchHarness {
    pub(crate) cache: FailingBatchCache<User>,
    runs_after_ok: Arc<AtomicUsize>,
    runs_after_fail: Arc<AtomicUsize>,
    runs_before_fail: Arc<AtomicUsize>,
    runs_propagate: Arc<AtomicUsize>,
}

impl CacheEvictBatchHarness {
    pub(crate) fn new() -> Self {
        Self {
            cache: FailingBatchCache::default(),
            runs_after_ok: Arc::new(AtomicUsize::new(0)),
            runs_after_fail: Arc::new(AtomicUsize::new(0)),
            runs_before_fail: Arc::new(AtomicUsize::new(0)),
            runs_propagate: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = false)]
    pub(crate) async fn evict_after_ok(&self, user_ids: Vec<u64>) -> MacroTestResult<()> {
        self.runs_after_ok.fetch_add(1, Ordering::SeqCst);
        let _ = user_ids;
        Ok(())
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = false)]
    pub(crate) async fn evict_after_fail(&self, user_ids: Vec<u64>) -> MacroTestResult<()> {
        self.runs_after_fail.fetch_add(1, Ordering::SeqCst);
        let _ = user_ids;
        Err(MacroTestError::Biz("after-fail"))
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = true)]
    pub(crate) async fn evict_before_fail(&self, user_ids: Vec<u64>) -> MacroTestResult<()> {
        self.runs_before_fail.fetch_add(1, Ordering::SeqCst);
        let _ = user_ids;
        Err(MacroTestError::Biz("before-fail"))
    }

    #[cache_evict_batch(
        cache = self.cache,
        keys = user_ids,
        before = false,
        on_cache_error = "propagate"
    )]
    pub(crate) async fn evict_propagate(&self, user_ids: Vec<u64>) -> MacroTestResult<()> {
        self.runs_propagate.fetch_add(1, Ordering::SeqCst);
        let _ = user_ids;
        Ok(())
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = false)]
    pub(crate) async fn evict_after_ok_with_slice(&self, user_ids: &[u64]) -> MacroTestResult<()> {
        self.runs_after_ok.fetch_add(1, Ordering::SeqCst);
        let _ = user_ids;
        Ok(())
    }

    pub(crate) fn runs_after_ok(&self) -> usize {
        self.runs_after_ok.load(Ordering::SeqCst)
    }

    pub(crate) fn runs_after_fail(&self) -> usize {
        self.runs_after_fail.load(Ordering::SeqCst)
    }

    pub(crate) fn runs_before_fail(&self) -> usize {
        self.runs_before_fail.load(Ordering::SeqCst)
    }

    pub(crate) fn runs_propagate(&self) -> usize {
        self.runs_propagate.load(Ordering::SeqCst)
    }
}
