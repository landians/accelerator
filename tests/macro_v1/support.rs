use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::CacheMode;
use accelerator::local;
use accelerator::macros::{cache_evict, cache_put, cacheable};
use accelerator::{CacheError, CacheResult, LevelCache};

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

/// A test cache that can inject get/set/del failures for macro behavior tests.
#[derive(Clone)]
pub(crate) struct FailingCache<V>
where
    V: Clone + Send + 'static,
{
    store: Arc<Mutex<HashMap<u64, Option<V>>>>,
    fail_get: Arc<AtomicBool>,
    fail_set: Arc<AtomicBool>,
    fail_del: Arc<AtomicBool>,
    get_calls: Arc<AtomicUsize>,
    set_calls: Arc<AtomicUsize>,
    del_calls: Arc<AtomicUsize>,
}

impl<V> Default for FailingCache<V>
where
    V: Clone + Send + 'static,
{
    fn default() -> Self {
        Self {
            store: Arc::new(Mutex::new(HashMap::new())),
            fail_get: Arc::new(AtomicBool::new(false)),
            fail_set: Arc::new(AtomicBool::new(false)),
            fail_del: Arc::new(AtomicBool::new(false)),
            get_calls: Arc::new(AtomicUsize::new(0)),
            set_calls: Arc::new(AtomicUsize::new(0)),
            del_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl<V> FailingCache<V>
where
    V: Clone + Send + 'static,
{
    pub(crate) fn set_fail_get(&self, enabled: bool) {
        self.fail_get.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_set(&self, enabled: bool) {
        self.fail_set.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn set_fail_del(&self, enabled: bool) {
        self.fail_del.store(enabled, Ordering::SeqCst);
    }

    pub(crate) fn get_calls(&self) -> usize {
        self.get_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn set_calls(&self) -> usize {
        self.set_calls.load(Ordering::SeqCst)
    }

    pub(crate) fn del_calls(&self) -> usize {
        self.del_calls.load(Ordering::SeqCst)
    }

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

    pub(crate) async fn get(&self, key: &u64, _opts: &ReadOptions) -> CacheResult<Option<V>> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock get failed".to_string()));
        }
        Ok(self.store.lock().unwrap().get(key).cloned().unwrap_or(None))
    }

    pub(crate) async fn set(&self, key: &u64, value: Option<V>) -> CacheResult<()> {
        self.set_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_set.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock set failed".to_string()));
        }
        self.store.lock().unwrap().insert(*key, value);
        Ok(())
    }

    pub(crate) async fn del(&self, key: &u64) -> CacheResult<()> {
        self.del_calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_del.load(Ordering::SeqCst) {
            return Err(CacheError::Backend("mock del failed".to_string()));
        }
        self.store.lock().unwrap().remove(key);
        Ok(())
    }
}

pub(crate) struct UserService {
    cache: LevelCache<u64, User>,
    query_count: Arc<AtomicUsize>,
    save_count: Arc<AtomicUsize>,
    delete_count: Arc<AtomicUsize>,
}

impl UserService {
    pub(crate) fn new() -> Self {
        let local_backend = local::moka::<User>().max_capacity(256).build().unwrap();
        let cache = LevelCacheBuilder::<u64, User>::new()
            .area("macro-test")
            .mode(CacheMode::Local)
            .local(local_backend)
            .local_ttl(Duration::from_secs(60))
            .null_ttl(Duration::from_secs(10))
            .build()
            .unwrap();

        Self {
            cache,
            query_count: Arc::new(AtomicUsize::new(0)),
            save_count: Arc::new(AtomicUsize::new(0)),
            delete_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cacheable(cache = self.cache, key = user_id)]
    pub(crate) async fn query_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.query_count.fetch_add(1, Ordering::SeqCst);
        Ok(Some(User {
            id: user_id,
            name: format!("user-{user_id}"),
        }))
    }

    #[cache_put(cache = self.cache, key = user.id, value = Some(user.clone()))]
    pub(crate) async fn save_user(&self, user: User) -> CacheResult<()> {
        self.save_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[cache_evict(cache = self.cache, key = user_id)]
    pub(crate) async fn delete_user(&self, user_id: u64) -> CacheResult<()> {
        self.delete_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[cache_evict(cache = self.cache, key = user_id, before = true)]
    pub(crate) async fn delete_user_before_and_fail(&self, user_id: u64) -> CacheResult<()> {
        self.delete_count.fetch_add(1, Ordering::SeqCst);
        Err(CacheError::Loader(format!("delete {user_id} failed")))
    }

    pub(crate) fn query_count(&self) -> usize {
        self.query_count.load(Ordering::SeqCst)
    }

    pub(crate) fn save_count(&self) -> usize {
        self.save_count.load(Ordering::SeqCst)
    }

    pub(crate) fn delete_count(&self) -> usize {
        self.delete_count.load(Ordering::SeqCst)
    }

    pub(crate) async fn seed_cache(&self, key: u64, value: Option<User>) {
        self.cache.set(&key, value).await.unwrap();
    }

    pub(crate) async fn cache_only_get(&self, key: u64) -> Option<User> {
        self.cache
            .get(
                &key,
                &ReadOptions {
                    allow_stale: false,
                    disable_load: true,
                },
            )
            .await
            .unwrap()
    }
}

pub(crate) struct CacheableHarness {
    pub(crate) cache: FailingCache<User>,
    hits_ignore: Arc<AtomicUsize>,
    hits_propagate: Arc<AtomicUsize>,
    hits_none_true: Arc<AtomicUsize>,
    hits_none_false: Arc<AtomicUsize>,
}

impl CacheableHarness {
    pub(crate) fn new() -> Self {
        Self {
            cache: FailingCache::default(),
            hits_ignore: Arc::new(AtomicUsize::new(0)),
            hits_propagate: Arc::new(AtomicUsize::new(0)),
            hits_none_true: Arc::new(AtomicUsize::new(0)),
            hits_none_false: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cacheable(cache = self.cache, key = user_id, on_cache_error = "ignore")]
    pub(crate) async fn get_ignore(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.hits_ignore.fetch_add(1, Ordering::SeqCst);
        Ok(Some(User {
            id: user_id,
            name: format!("ignore-{user_id}"),
        }))
    }

    #[cacheable(cache = self.cache, key = user_id, on_cache_error = "propagate")]
    pub(crate) async fn get_propagate(&self, user_id: u64) -> MacroTestResult<Option<User>> {
        self.hits_propagate.fetch_add(1, Ordering::SeqCst);
        Ok(Some(User {
            id: user_id,
            name: format!("propagate-{user_id}"),
        }))
    }

    #[cacheable(cache = self.cache, key = user_id, cache_none = true)]
    pub(crate) async fn get_none_with_cache(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.hits_none_true.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
        Ok(None::<User>)
    }

    #[cacheable(cache = self.cache, key = user_id, cache_none = false)]
    pub(crate) async fn get_none_without_cache(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.hits_none_false.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
        Ok(None::<User>)
    }

    pub(crate) fn hits_ignore(&self) -> usize {
        self.hits_ignore.load(Ordering::SeqCst)
    }

    pub(crate) fn hits_propagate(&self) -> usize {
        self.hits_propagate.load(Ordering::SeqCst)
    }

    pub(crate) fn hits_none_true(&self) -> usize {
        self.hits_none_true.load(Ordering::SeqCst)
    }

    pub(crate) fn hits_none_false(&self) -> usize {
        self.hits_none_false.load(Ordering::SeqCst)
    }
}

pub(crate) struct CachePutHarness {
    pub(crate) cache: FailingCache<User>,
    runs_ignore: Arc<AtomicUsize>,
    runs_propagate: Arc<AtomicUsize>,
}

impl CachePutHarness {
    pub(crate) fn new() -> Self {
        Self {
            cache: FailingCache::default(),
            runs_ignore: Arc::new(AtomicUsize::new(0)),
            runs_propagate: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cache_put(
        cache = self.cache,
        key = user.id,
        value = Some(user.clone()),
        on_cache_error = "ignore"
    )]
    pub(crate) async fn put_ignore(&self, user: User) -> CacheResult<()> {
        self.runs_ignore.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[cache_put(
        cache = self.cache,
        key = user.id,
        value = Some(user.clone()),
        on_cache_error = "propagate"
    )]
    pub(crate) async fn put_propagate(&self, user: User) -> MacroTestResult<()> {
        self.runs_propagate.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) fn runs_ignore(&self) -> usize {
        self.runs_ignore.load(Ordering::SeqCst)
    }

    pub(crate) fn runs_propagate(&self) -> usize {
        self.runs_propagate.load(Ordering::SeqCst)
    }
}

pub(crate) struct CacheEvictHarness {
    pub(crate) cache: FailingCache<User>,
    runs_after_ok: Arc<AtomicUsize>,
    runs_after_fail: Arc<AtomicUsize>,
    runs_before_fail: Arc<AtomicUsize>,
    runs_propagate: Arc<AtomicUsize>,
}

impl CacheEvictHarness {
    pub(crate) fn new() -> Self {
        Self {
            cache: FailingCache::default(),
            runs_after_ok: Arc::new(AtomicUsize::new(0)),
            runs_after_fail: Arc::new(AtomicUsize::new(0)),
            runs_before_fail: Arc::new(AtomicUsize::new(0)),
            runs_propagate: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[cache_evict(cache = self.cache, key = user_id, before = false)]
    pub(crate) async fn evict_after_ok(&self, user_id: u64) -> MacroTestResult<()> {
        self.runs_after_ok.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
        Ok(())
    }

    #[cache_evict(cache = self.cache, key = user_id, before = false)]
    pub(crate) async fn evict_after_fail(&self, user_id: u64) -> MacroTestResult<()> {
        self.runs_after_fail.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
        Err(MacroTestError::Biz("after-fail"))
    }

    #[cache_evict(cache = self.cache, key = user_id, before = true)]
    pub(crate) async fn evict_before_fail(&self, user_id: u64) -> MacroTestResult<()> {
        self.runs_before_fail.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
        Err(MacroTestError::Biz("before-fail"))
    }

    #[cache_evict(
        cache = self.cache,
        key = user_id,
        before = false,
        on_cache_error = "propagate"
    )]
    pub(crate) async fn evict_propagate(&self, user_id: u64) -> MacroTestResult<()> {
        self.runs_propagate.fetch_add(1, Ordering::SeqCst);
        let _ = user_id;
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
