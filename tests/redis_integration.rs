use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use accelerator::backend::{StoredEntry, StoredValue};
use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::CacheMode;
use accelerator::{local, remote};
use redis::AsyncCommands;

fn redis_url() -> String {
    std::env::var("ACCELERATOR_TEST_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
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

    let pong: redis::RedisResult<String> = conn.ping().await;
    pong.is_ok()
}

fn unique_scope(tag: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{tag}-{}-{nanos}", std::process::id())
}

async fn skip_if_redis_unavailable(test_name: &str) -> Option<String> {
    let url = redis_url();
    if redis_ready(&url).await {
        Some(url)
    } else {
        eprintln!("skip `{test_name}`: redis is not reachable at {url}");
        None
    }
}

#[tokio::test]
async fn redis_backend_roundtrip_and_batch_ops() {
    let Some(url) = skip_if_redis_unavailable("redis_backend_roundtrip_and_batch_ops").await else {
        return;
    };

    let scope = unique_scope("remote-roundtrip");
    let backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    backend
        .set(
            "k1",
            StoredEntry {
                value: StoredValue::Value("v1".to_string()),
                expire_at: Instant::now() + Duration::from_secs(30),
            },
        )
        .await
        .unwrap();

    let mut mset_map = HashMap::new();
    mset_map.insert(
        "k2".to_string(),
        StoredEntry {
            value: StoredValue::Value("v2".to_string()),
            expire_at: Instant::now() + Duration::from_secs(30),
        },
    );
    mset_map.insert(
        "k3".to_string(),
        StoredEntry {
            value: StoredValue::Null,
            expire_at: Instant::now() + Duration::from_secs(30),
        },
    );
    backend.mset(mset_map).await.unwrap();

    let one = backend.get("k1").await.unwrap();
    assert!(matches!(
        one.map(|entry| entry.value),
        Some(StoredValue::Value(value)) if value == "v1"
    ));

    let batch = backend
        .mget(&[
            "k1".to_string(),
            "k2".to_string(),
            "k3".to_string(),
            "k4".to_string(),
        ])
        .await
        .unwrap();

    assert_eq!(batch.len(), 4);
    assert!(matches!(
        batch.get("k2").cloned().flatten().map(|entry| entry.value),
        Some(StoredValue::Value(value)) if value == "v2"
    ));
    assert!(matches!(
        batch.get("k3").cloned().flatten().map(|entry| entry.value),
        Some(StoredValue::Null)
    ));
    assert!(batch.get("k4").cloned().flatten().is_none());

    backend.del("k1").await.unwrap();
    backend
        .mdel(&["k2".to_string(), "k3".to_string(), "k4".to_string()])
        .await
        .unwrap();

    let after = backend
        .mget(&["k1".to_string(), "k2".to_string(), "k3".to_string()])
        .await
        .unwrap();
    assert!(after.values().all(|value| value.is_none()));
}

#[tokio::test]
async fn level_cache_both_mode_remote_hit_backfills_local() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_both_mode_remote_hit_backfills_local").await
    else {
        return;
    };

    let scope = unique_scope("cache-both-backfill");
    let area = format!("area-{scope}");
    let local_backend = local::moka::<String>().max_capacity(128).build().unwrap();
    let remote_backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    let cache = LevelCacheBuilder::<u64, String>::new()
        .area(area.clone())
        .mode(CacheMode::Both)
        .local(local_backend.clone())
        .remote(remote_backend.clone())
        .local_ttl(Duration::from_secs(60))
        .remote_ttl(Duration::from_secs(120))
        .build()
        .unwrap();

    cache.set(&42, Some("answer".to_string())).await.unwrap();

    let encoded = format!("{area}:42");
    local_backend.del(&encoded).await.unwrap();

    let value = cache.get(&42, &ReadOptions::default()).await.unwrap();
    assert_eq!(value, Some("answer".to_string()));

    let local_after = local_backend.get(&encoded).await.unwrap();
    assert!(local_after.is_some());
}

#[tokio::test]
async fn level_cache_remote_mode_loader_and_read_options() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_remote_mode_loader_and_read_options").await
    else {
        return;
    };

    let scope = unique_scope("cache-remote-loader");
    let area = format!("area-{scope}");
    let remote_backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    let loads = Arc::new(AtomicUsize::new(0));
    let loads_for_loader = loads.clone();
    let cache = LevelCacheBuilder::<u64, String>::new()
        .area(area)
        .mode(CacheMode::Remote)
        .remote(remote_backend)
        .loader_fn(move |key: u64| {
            let loads_for_loader = loads_for_loader.clone();
            async move {
                loads_for_loader.fetch_add(1, Ordering::SeqCst);
                Ok(Some(format!("remote-{key}")))
            }
        })
        .build()
        .unwrap();

    let disabled = cache
        .get(
            &7,
            &ReadOptions {
                allow_stale: false,
                disable_load: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(disabled, None);
    assert_eq!(loads.load(Ordering::SeqCst), 0);

    let first = cache.get(&7, &ReadOptions::default()).await.unwrap();
    let second = cache.get(&7, &ReadOptions::default()).await.unwrap();
    assert_eq!(first, Some("remote-7".to_string()));
    assert_eq!(second, Some("remote-7".to_string()));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn level_cache_mget_covers_all_keys_and_writes_back() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_mget_covers_all_keys_and_writes_back").await
    else {
        return;
    };

    let scope = unique_scope("cache-mget");
    let area = format!("area-{scope}");
    let local_backend = local::moka::<String>().build().unwrap();
    let remote_backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    let cache = LevelCacheBuilder::<u64, String>::new()
        .area(area.clone())
        .mode(CacheMode::Both)
        .local(local_backend)
        .remote(remote_backend.clone())
        .loader_fn(|key: u64| async move {
            match key {
                2 => Ok(Some("two-loaded".to_string())),
                3 => Ok(None),
                _ => Ok(None),
            }
        })
        .build()
        .unwrap();

    cache.set(&1, Some("one".to_string())).await.unwrap();

    let values = cache
        .mget(&[1, 2, 3, 4], &ReadOptions::default())
        .await
        .unwrap();

    assert_eq!(values.len(), 4);
    assert_eq!(values.get(&1).cloned().flatten(), Some("one".to_string()));
    assert_eq!(
        values.get(&2).cloned().flatten(),
        Some("two-loaded".to_string())
    );
    assert_eq!(values.get(&3).cloned().flatten(), None);
    assert_eq!(values.get(&4).cloned().flatten(), None);

    let encoded_two = format!("{area}:2");
    let remote_two = remote_backend.get(&encoded_two).await.unwrap();
    assert!(remote_two.is_some());
}

#[tokio::test]
async fn level_cache_singleflight_works_with_real_redis_backend() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_singleflight_works_with_real_redis_backend").await
    else {
        return;
    };

    let scope = unique_scope("cache-singleflight");
    let remote_backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope)
        .build()
        .unwrap();

    let loads = Arc::new(AtomicUsize::new(0));
    let loads_for_loader = loads.clone();
    let cache = Arc::new(
        LevelCacheBuilder::<u64, String>::new()
            .area("singleflight")
            .mode(CacheMode::Remote)
            .remote(remote_backend)
            .loader_fn(move |key: u64| {
                let loads_for_loader = loads_for_loader.clone();
                async move {
                    loads_for_loader.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    Ok(Some(format!("sf-{key}")))
                }
            })
            .build()
            .unwrap(),
    );

    let tasks = (0..24)
        .map(|_| {
            let cache = cache.clone();
            tokio::spawn(async move { cache.get(&99, &ReadOptions::default()).await })
        })
        .collect::<Vec<_>>();

    for task in tasks {
        let value = task.await.unwrap().unwrap();
        assert_eq!(value, Some("sf-99".to_string()));
    }
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn level_cache_loader_timeout_is_reported_with_remote_backend() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_loader_timeout_is_reported_with_remote_backend")
            .await
    else {
        return;
    };

    let scope = unique_scope("cache-timeout");
    let remote_backend = remote::redis::<String>()
        .url(url)
        .key_prefix(scope)
        .build()
        .unwrap();

    let cache = LevelCacheBuilder::<u64, String>::new()
        .area("loader-timeout")
        .mode(CacheMode::Remote)
        .remote(remote_backend)
        .loader_timeout(Duration::from_millis(5))
        .loader_fn(|_key: u64| async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(Some("late".to_string()))
        })
        .build()
        .unwrap();

    let err = cache.get(&1, &ReadOptions::default()).await.unwrap_err();
    assert!(matches!(err, accelerator::CacheError::Timeout("loader")));
}

#[tokio::test]
async fn level_cache_broadcast_invalidation_clears_other_instance_l1() {
    let Some(url) =
        skip_if_redis_unavailable("level_cache_broadcast_invalidation_clears_other_instance_l1")
            .await
    else {
        return;
    };

    let scope = unique_scope("cache-invalidation-broadcast");
    let area = format!("area-{scope}");

    let local_a = local::moka::<String>().max_capacity(128).build().unwrap();
    let remote_a = remote::redis::<String>()
        .url(url.clone())
        .key_prefix(scope.clone())
        .build()
        .unwrap();

    let local_b = local::moka::<String>().max_capacity(128).build().unwrap();
    let remote_b = remote::redis::<String>()
        .url(url)
        .key_prefix(scope)
        .build()
        .unwrap();

    let cache_a = LevelCacheBuilder::<u64, String>::new()
        .area(area.clone())
        .mode(CacheMode::Both)
        .local(local_a)
        .remote(remote_a)
        .broadcast_invalidation(true)
        .build()
        .unwrap();

    let cache_b = LevelCacheBuilder::<u64, String>::new()
        .area(area.clone())
        .mode(CacheMode::Both)
        .local(local_b.clone())
        .remote(remote_b)
        .broadcast_invalidation(true)
        .build()
        .unwrap();

    cache_a
        .set(&88, Some("broadcast".to_string()))
        .await
        .unwrap();
    let _ = cache_b.get(&88, &ReadOptions::default()).await.unwrap();

    let encoded = format!("{area}:88");
    let local_before = local_b.get(&encoded).await.unwrap();
    assert!(local_before.is_some());

    tokio::time::sleep(Duration::from_millis(50)).await;
    cache_a.del(&88).await.unwrap();

    tokio::time::sleep(Duration::from_millis(120)).await;
    let local_after = local_b.get(&encoded).await.unwrap();
    assert!(local_after.is_none());

    let read_after = cache_b.get(&88, &ReadOptions::default()).await.unwrap();
    assert_eq!(read_after, None);

    let metrics_b = cache_b.metrics_snapshot();
    assert!(metrics_b.invalidation_receive >= 1);
}
