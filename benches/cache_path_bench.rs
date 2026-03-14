use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::CacheMode;
use accelerator::local;
use accelerator::macros::{cacheable, cacheable_batch};
use accelerator::remote;
use accelerator::{CacheResult, LevelCache};
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use redis::AsyncCommands;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct User {
    id: u64,
    name: String,
}

#[derive(Clone, Default)]
struct BenchRepo;

impl BenchRepo {
    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        Ok(Some(User {
            id: user_id,
            name: format!("user-{user_id}"),
        }))
    }

    async fn batch_get_users(&self, user_ids: &[u64]) -> CacheResult<HashMap<u64, Option<User>>> {
        let mut values = HashMap::with_capacity(user_ids.len());
        for user_id in user_ids {
            values.insert(*user_id, self.get_user(*user_id).await?);
        }
        Ok(values)
    }
}

fn build_local_cache(area: &str) -> LevelCache<u64, User> {
    let local_backend = local::moka::<User>().max_capacity(200_000).build().unwrap();

    LevelCacheBuilder::<u64, User>::new()
        .area(area)
        .mode(CacheMode::Local)
        .local(local_backend)
        .local_ttl(Duration::from_secs(60))
        .null_ttl(Duration::from_secs(10))
        .build()
        .unwrap()
}

fn build_remote_cache(area: &str, key_prefix: &str, redis_url: &str) -> LevelCache<u64, User> {
    let remote_backend = remote::redis::<User>()
        .url(redis_url)
        .key_prefix(key_prefix)
        .build()
        .unwrap();

    LevelCacheBuilder::<u64, User>::new()
        .area(area)
        .mode(CacheMode::Remote)
        .remote(remote_backend)
        .remote_ttl(Duration::from_secs(60))
        .null_ttl(Duration::from_secs(10))
        .build()
        .unwrap()
}

async fn redis_ready(redis_url: &str) -> bool {
    let client = match redis::Client::open(redis_url) {
        Ok(client) => client,
        Err(_) => return false,
    };

    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(_) => return false,
    };

    conn.ping::<String>().await.is_ok()
}

struct HandwrittenService {
    cache: LevelCache<u64, User>,
    repo: BenchRepo,
}

impl HandwrittenService {
    fn new(area: &str) -> Self {
        Self::new_with_cache(build_local_cache(area))
    }

    fn new_with_cache(cache: LevelCache<u64, User>) -> Self {
        Self {
            cache,
            repo: BenchRepo,
        }
    }

    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        let options = ReadOptions {
            allow_stale: false,
            disable_load: true,
        };

        if let Some(hit) = self.cache.get(&user_id, &options).await? {
            return Ok(Some(hit));
        }

        let loaded = self.repo.get_user(user_id).await?;
        if let Some(value) = loaded.as_ref() {
            self.cache.set(&user_id, Some(value.clone())).await?;
        }

        Ok(loaded)
    }

    async fn batch_get_users(&self, user_ids: Vec<u64>) -> CacheResult<HashMap<u64, Option<User>>> {
        if user_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let options = ReadOptions {
            allow_stale: false,
            disable_load: true,
        };

        let mut values = self.cache.mget(&user_ids, &options).await?;
        let misses = user_ids
            .iter()
            .filter(|user_id| match values.get(*user_id) {
                Some(value) => value.is_none(),
                None => true,
            })
            .copied()
            .collect::<Vec<_>>();

        if misses.is_empty() {
            return Ok(values);
        }

        let loaded = self.repo.batch_get_users(&misses).await?;
        let mut writes = HashMap::with_capacity(misses.len());

        for user_id in misses {
            let value = loaded.get(&user_id).cloned().unwrap_or(None);
            values.insert(user_id, value.clone());
            writes.insert(user_id, value);
        }

        self.cache.mset(writes).await?;
        Ok(values)
    }
}

struct MacroService {
    cache: LevelCache<u64, User>,
    repo: BenchRepo,
}

impl MacroService {
    fn new(area: &str) -> Self {
        Self::new_with_cache(build_local_cache(area))
    }

    fn new_with_cache(cache: LevelCache<u64, User>) -> Self {
        Self {
            cache,
            repo: BenchRepo,
        }
    }

    #[cacheable(cache = self.cache, key = user_id)]
    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.repo.get_user(user_id).await
    }

    #[cacheable_batch(cache = self.cache, keys = user_ids)]
    async fn batch_get_users(&self, user_ids: Vec<u64>) -> CacheResult<HashMap<u64, Option<User>>> {
        self.repo.batch_get_users(&user_ids).await
    }
}

fn bench_single_key(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let manual = Arc::new(HandwrittenService::new("bench-single-manual"));
    let macros = Arc::new(MacroService::new("bench-single-macro"));

    runtime.block_on(async {
        manual.get_user(42).await.unwrap();
        macros.get_user(42).await.unwrap();
    });

    let miss_seed = Arc::new(AtomicU64::new(1_000_000));

    let mut group = c.benchmark_group("single_key");

    group.bench_function("hit/manual", |b| {
        let manual = manual.clone();
        b.to_async(&runtime)
            .iter(|| async { manual.get_user(42).await.unwrap() });
    });

    group.bench_function("hit/macro", |b| {
        let macros = macros.clone();
        b.to_async(&runtime)
            .iter(|| async { macros.get_user(42).await.unwrap() });
    });

    group.bench_function("miss/manual", |b| {
        let manual = manual.clone();
        let seed = miss_seed.clone();
        b.to_async(&runtime).iter(|| async {
            let key = seed.fetch_add(1, Ordering::Relaxed);
            manual.get_user(key).await.unwrap()
        });
    });

    group.bench_function("miss/macro", |b| {
        let macros = macros.clone();
        let seed = miss_seed.clone();
        b.to_async(&runtime).iter(|| async {
            let key = seed.fetch_add(1, Ordering::Relaxed);
            macros.get_user(key).await.unwrap()
        });
    });

    group.finish();

    let redis_url = std::env::var("ACCELERATOR_BENCH_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    if runtime.block_on(redis_ready(&redis_url)) {
        let key_prefix = "accelerator-bench-single-remote";
        let remote_manual = Arc::new(HandwrittenService::new_with_cache(build_remote_cache(
            "bench-single-remote-manual",
            key_prefix,
            &redis_url,
        )));
        let remote_macro = Arc::new(MacroService::new_with_cache(build_remote_cache(
            "bench-single-remote-macro",
            key_prefix,
            &redis_url,
        )));

        runtime.block_on(async {
            remote_manual.get_user(9001).await.unwrap();
            remote_macro.get_user(9002).await.unwrap();
        });

        let mut remote_group = c.benchmark_group("single_key_remote");
        remote_group.bench_function("hit/manual", |b| {
            let remote_manual = remote_manual.clone();
            b.to_async(&runtime)
                .iter(|| async { remote_manual.get_user(9001).await.unwrap() });
        });
        remote_group.bench_function("hit/macro", |b| {
            let remote_macro = remote_macro.clone();
            b.to_async(&runtime)
                .iter(|| async { remote_macro.get_user(9002).await.unwrap() });
        });
        remote_group.finish();
    }
}

fn bench_batch(c: &mut Criterion) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    let manual = Arc::new(HandwrittenService::new("bench-batch-manual"));
    let macros = Arc::new(MacroService::new("bench-batch-macro"));

    runtime.block_on(async {
        let keys = (1..=32).collect::<Vec<_>>();
        manual.batch_get_users(keys.clone()).await.unwrap();
        macros.batch_get_users(keys).await.unwrap();
    });

    let miss_seed = Arc::new(AtomicU64::new(2_000_000));

    let mut group = c.benchmark_group("batch");
    group.throughput(Throughput::Elements(32));

    group.bench_with_input(BenchmarkId::new("hit/manual", 32), &32usize, |b, size| {
        let manual = manual.clone();
        b.to_async(&runtime).iter(|| async {
            let keys = (1..=*size as u64).collect::<Vec<_>>();
            manual.batch_get_users(keys).await.unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("hit/macro", 32), &32usize, |b, size| {
        let macros = macros.clone();
        b.to_async(&runtime).iter(|| async {
            let keys = (1..=*size as u64).collect::<Vec<_>>();
            macros.batch_get_users(keys).await.unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("miss/manual", 32), &32usize, |b, size| {
        let manual = manual.clone();
        let seed = miss_seed.clone();
        b.to_async(&runtime).iter(|| async {
            let start = seed.fetch_add(*size as u64, Ordering::Relaxed);
            let keys = (start..start + *size as u64).collect::<Vec<_>>();
            manual.batch_get_users(keys).await.unwrap()
        });
    });

    group.bench_with_input(BenchmarkId::new("miss/macro", 32), &32usize, |b, size| {
        let macros = macros.clone();
        let seed = miss_seed.clone();
        b.to_async(&runtime).iter(|| async {
            let start = seed.fetch_add(*size as u64, Ordering::Relaxed);
            let keys = (start..start + *size as u64).collect::<Vec<_>>();
            macros.batch_get_users(keys).await.unwrap()
        });
    });

    group.finish();
}

criterion_group!(
    name = benches;
    config = Criterion::default().sample_size(60);
    targets = bench_single_key, bench_batch
);
criterion_main!(benches);
