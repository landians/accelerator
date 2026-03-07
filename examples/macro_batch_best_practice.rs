use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::config::CacheMode;
use accelerator::local;
use accelerator::macros::{cache_evict_batch, cacheable_batch};
use accelerator::{CacheResult, LevelCache};

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
    async fn batch_find(&self, user_ids: &[u64]) -> CacheResult<HashMap<u64, Option<User>>> {
        let table = self.table.lock().unwrap();
        let mut values = HashMap::with_capacity(user_ids.len());
        for user_id in user_ids {
            values.insert(*user_id, table.get(user_id).cloned());
        }
        Ok(values)
    }

    async fn batch_delete(&self, user_ids: &[u64]) -> CacheResult<()> {
        let mut table = self.table.lock().unwrap();
        for user_id in user_ids {
            table.remove(user_id);
        }
        Ok(())
    }

    fn seed(&self, user: User) {
        self.table.lock().unwrap().insert(user.id, user);
    }
}

struct UserService {
    cache: LevelCache<u64, User>,
    repo: UserRepo,
}

impl UserService {
    fn new(cache: LevelCache<u64, User>, repo: UserRepo) -> Self {
        Self { cache, repo }
    }

    #[cacheable_batch(cache = self.cache, keys = user_ids, allow_stale = false)]
    async fn batch_get(&self, user_ids: Vec<u64>) -> CacheResult<HashMap<u64, Option<User>>> {
        self.repo.batch_find(&user_ids).await
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = false)]
    async fn batch_delete(&self, user_ids: Vec<u64>) -> CacheResult<()> {
        self.repo.batch_delete(&user_ids).await
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let local_backend = local::moka::<User>().max_capacity(20_000).build()?;
    let cache = LevelCacheBuilder::<u64, User>::new()
        .area("macro-batch-demo")
        .mode(CacheMode::Local)
        .local(local_backend)
        .local_ttl(Duration::from_secs(90))
        .null_ttl(Duration::from_secs(30))
        .build()?;

    let repo = UserRepo::default();
    repo.seed(User {
        id: 101,
        name: "alice".to_string(),
    });
    repo.seed(User {
        id: 102,
        name: "bob".to_string(),
    });

    let service = UserService::new(cache, repo);

    let first = service.batch_get(vec![101, 102, 999]).await?;
    let second = service.batch_get(vec![101, 102, 999]).await?;
    println!("first={first:?}");
    println!("second={second:?}");

    service.batch_delete(vec![101, 102]).await?;
    let after_delete = service.batch_get(vec![101, 102]).await?;
    println!("after_delete={after_delete:?}");

    Ok(())
}
