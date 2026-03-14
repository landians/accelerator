use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::CacheMode;
use accelerator::loader::{Loader, MLoader};
use accelerator::{CacheError, CacheResult, local, remote};
use redis::AsyncCommands;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct DbUser {
    id: u64,
    name: String,
}

#[derive(Clone)]
struct PgUserLoader {
    pool: PgPool,
    load_calls: Arc<AtomicUsize>,
    mload_calls: Arc<AtomicUsize>,
}

impl PgUserLoader {
    fn new(pool: PgPool) -> Self {
        Self {
            pool,
            load_calls: Arc::new(AtomicUsize::new(0)),
            mload_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn load_calls(&self) -> usize {
        self.load_calls.load(Ordering::SeqCst)
    }

    fn mload_calls(&self) -> usize {
        self.mload_calls.load(Ordering::SeqCst)
    }
}

impl Loader<u64, DbUser> for PgUserLoader {
    async fn load(&self, key: &u64) -> CacheResult<Option<DbUser>> {
        self.load_calls.fetch_add(1, Ordering::SeqCst);

        let row = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, name FROM accelerator_users WHERE id = $1",
        )
        .bind(*key as i64)
        .fetch_optional(&self.pool)
        .await
        .map_err(|err| CacheError::Loader(format!("sqlx load failed: {err}")))?;

        Ok(row.map(|(id, name)| DbUser {
            id: id as u64,
            name,
        }))
    }
}

impl MLoader<u64, DbUser> for PgUserLoader {
    async fn mload(&self, keys: &[u64]) -> CacheResult<HashMap<u64, Option<DbUser>>> {
        self.mload_calls.fetch_add(1, Ordering::SeqCst);

        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = keys.iter().map(|key| *key as i64).collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, name FROM accelerator_users WHERE id = ANY($1::bigint[])",
        )
        .bind(&ids)
        .fetch_all(&self.pool)
        .await
        .map_err(|err| CacheError::Loader(format!("sqlx mload failed: {err}")))?;

        let mut found = HashMap::with_capacity(rows.len());
        for (id, name) in rows {
            found.insert(
                id as u64,
                DbUser {
                    id: id as u64,
                    name,
                },
            );
        }

        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(*key, found.get(key).cloned());
        }

        Ok(values)
    }
}

fn redis_url() -> String {
    std::env::var("ACCELERATOR_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

fn postgres_dsn() -> String {
    std::env::var("ACCELERATOR_TEST_POSTGRES_DSN").unwrap_or_else(|_| {
        "postgres://accelerator:accelerator@127.0.0.1:5432/accelerator".to_string()
    })
}

async fn redis_ready(url: &str) -> bool {
    let client = match redis::Client::open(url) {
        Ok(client) => client,
        Err(_) => return false,
    };

    let mut conn = match client.get_multiplexed_async_connection().await {
        Ok(conn) => conn,
        Err(_) => return false,
    };

    conn.ping::<String>().await.is_ok()
}

async fn postgres_connect(dsn: &str) -> Option<PgPool> {
    PgPoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(2))
        .connect(dsn)
        .await
        .ok()
}

fn unique_scope(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{tag}-{}-{nanos}", std::process::id())
}

async fn skip_if_stack_unavailable(test_name: &str) -> Option<(String, PgPool)> {
    let redis = redis_url();
    if !redis_ready(&redis).await {
        eprintln!("skip `{test_name}`: redis is not reachable at {redis}");
        return None;
    }

    let dsn = postgres_dsn();
    let Some(pg_client) = postgres_connect(&dsn).await else {
        eprintln!("skip `{test_name}`: postgres is not reachable with dsn `{dsn}`");
        return None;
    };

    Some((redis, pg_client))
}

#[tokio::test]
async fn cache_component_works_with_redis_and_postgres_loader() {
    let Some((redis, pg_pool)) =
        skip_if_stack_unavailable("cache_component_works_with_redis_and_postgres_loader").await
    else {
        return;
    };

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS accelerator_users (id BIGINT PRIMARY KEY, name TEXT NOT NULL)",
    )
    .execute(&pg_pool)
    .await
    .unwrap();

    let user_1 = 91_001_i64;
    let user_2 = 91_002_i64;

    sqlx::query(
        "INSERT INTO accelerator_users(id, name) VALUES($1, $2)
         ON CONFLICT(id) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(user_1)
    .bind("alice-v1")
    .execute(&pg_pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO accelerator_users(id, name) VALUES($1, $2)
         ON CONFLICT(id) DO UPDATE SET name = EXCLUDED.name",
    )
    .bind(user_2)
    .bind("bob-v1")
    .execute(&pg_pool)
    .await
    .unwrap();

    let scope = unique_scope("stack-it");
    let area = format!("area-{scope}");
    let local_backend = local::moka::<DbUser>().max_capacity(128).build().unwrap();
    let remote_backend = remote::redis::<DbUser>()
        .url(redis)
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    let loader = PgUserLoader::new(pg_pool.clone());
    let loader_probe = loader.clone();

    let cache = LevelCacheBuilder::<u64, DbUser, PgUserLoader>::new()
        .area(area.clone())
        .mode(CacheMode::Both)
        .local(local_backend)
        .remote(remote_backend)
        .penetration_protect(false)
        .loader(loader)
        .local_ttl(Duration::from_secs(60))
        .remote_ttl(Duration::from_secs(120))
        .null_ttl(Duration::from_secs(10))
        .build()
        .unwrap();

    let first = cache
        .get(&(user_1 as u64), &ReadOptions::default())
        .await
        .unwrap();
    assert_eq!(
        first,
        Some(DbUser {
            id: user_1 as u64,
            name: "alice-v1".to_string()
        })
    );
    assert_eq!(loader_probe.load_calls(), 1);

    sqlx::query("UPDATE accelerator_users SET name = $2 WHERE id = $1")
        .bind(user_1)
        .bind("alice-v2")
        .execute(&pg_pool)
        .await
        .unwrap();

    let second = cache
        .get(&(user_1 as u64), &ReadOptions::default())
        .await
        .unwrap();
    assert_eq!(
        second,
        Some(DbUser {
            id: user_1 as u64,
            name: "alice-v1".to_string()
        })
    );
    assert_eq!(loader_probe.load_calls(), 1);

    let batch = cache
        .mget(
            &[user_1 as u64, user_2 as u64, 91_003_u64],
            &ReadOptions::default(),
        )
        .await
        .unwrap();

    assert_eq!(
        batch.get(&(user_1 as u64)).cloned().flatten(),
        Some(DbUser {
            id: user_1 as u64,
            name: "alice-v1".to_string()
        })
    );
    assert_eq!(
        batch.get(&(user_2 as u64)).cloned().flatten(),
        Some(DbUser {
            id: user_2 as u64,
            name: "bob-v1".to_string()
        })
    );
    assert_eq!(batch.get(&91_003_u64).cloned().flatten(), None);
    assert_eq!(loader_probe.mload_calls(), 1);

    let otel = cache.otel_metric_points();
    assert!(!otel.is_empty());
    assert!(
        otel.iter()
            .all(|point| { point.attributes == vec![("area", area.clone())] })
    );

    cache.del(&(user_1 as u64)).await.unwrap();
    let after_del = cache
        .get(
            &(user_1 as u64),
            &ReadOptions {
                allow_stale: false,
                disable_load: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(after_del, None);

    sqlx::query("DELETE FROM accelerator_users WHERE id = ANY($1::bigint[])")
        .bind(&vec![user_1, user_2])
        .execute(&pg_pool)
        .await
        .unwrap();
}
