use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::config::CacheMode;
use accelerator::local;
use accelerator::macros::{cache_evict, cache_put, cacheable};
use accelerator::{CacheError, CacheResult, LevelCache};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct User {
    id: u64,
    name: String,
}

#[derive(Clone, Default)]
struct UserRepo {
    table: Arc<Mutex<HashMap<u64, User>>>,
}

impl UserRepo {
    async fn find_by_id(&self, user_id: u64) -> CacheResult<Option<User>> {
        Ok(self.table.lock().unwrap().get(&user_id).cloned())
    }

    async fn upsert(&self, user: User) -> CacheResult<()> {
        self.table.lock().unwrap().insert(user.id, user);
        Ok(())
    }

    async fn delete(&self, user_id: u64) -> CacheResult<()> {
        self.table.lock().unwrap().remove(&user_id);
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
enum ServiceError {
    #[error(transparent)]
    Cache(#[from] CacheError),
}

type ServiceResult<T> = Result<T, ServiceError>;

struct UserService {
    cache: LevelCache<u64, User>,
    repo: UserRepo,
}

impl UserService {
    fn new(cache: LevelCache<u64, User>, repo: UserRepo) -> Self {
        Self { cache, repo }
    }

    #[cacheable(
        cache = self.cache,
        key = user_id,
        allow_stale = false,
        cache_none = true,
        on_cache_error = "ignore"
    )]
    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.repo.find_by_id(user_id).await
    }

    #[cache_put(
        cache = self.cache,
        key = user.id,
        value = Some(user.clone()),
        on_cache_error = "propagate"
    )]
    async fn save_user(&self, user: User) -> ServiceResult<()> {
        self.repo.upsert(user.clone()).await?;
        Ok(())
    }

    #[cache_evict(
        cache = self.cache,
        key = user_id,
        before = true,
        on_cache_error = "propagate"
    )]
    async fn delete_user(&self, user_id: u64) -> ServiceResult<()> {
        self.repo.delete(user_id).await?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local_backend = local::moka::<User>().max_capacity(10_000).build()?;
    let cache = LevelCacheBuilder::<u64, User>::new()
        .area("macro-demo")
        .mode(CacheMode::Local)
        .local(local_backend)
        .local_ttl(Duration::from_secs(120))
        .null_ttl(Duration::from_secs(15))
        .build()?;

    let repo = UserRepo::default();
    let service = UserService::new(cache, repo);

    service
        .save_user(User {
            id: 1001,
            name: "alice".to_string(),
        })
        .await?;

    let first = service.get_user(1001).await?;
    let second = service.get_user(1001).await?;
    println!("first={first:?}, second={second:?}");

    service.delete_user(1001).await?;
    let after_delete = service.get_user(1001).await?;
    println!("after_delete={after_delete:?}");

    Ok(())
}
