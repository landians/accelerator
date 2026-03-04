use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::{CacheMode, ReadValueMode};
use accelerator::{local, remote};
use redis::AsyncCommands;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct UserProfile {
    id: u64,
    name: String,
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let redis_url = std::env::var("ACCELERATOR_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    if !redis_ready(&redis_url).await {
        println!("Redis is not reachable at {redis_url}, skip running example.");
        return Ok(());
    }

    let l1 = local::moka::<UserProfile>().max_capacity(200_000).build()?;

    let l2 = remote::redis::<UserProfile>()
        .url(redis_url)
        .key_prefix("accel-demo")
        .build()?;

    let cache = LevelCacheBuilder::<u64, UserProfile>::new()
        .area("user_profile")
        .mode(CacheMode::Both)
        .local(l1)
        .remote(l2)
        .local_ttl(Duration::from_secs(60))
        .remote_ttl(Duration::from_secs(300))
        .null_ttl(Duration::from_secs(30))
        .copy_on_read(true)
        .copy_on_write(true)
        .read_value_mode(ReadValueMode::OwnedClone)
        .penetration_protect(true)
        .loader_timeout(Duration::from_millis(120))
        .loader_fn(|uid: u64| async move {
            Ok(Some(UserProfile {
                id: uid,
                name: format!("user-{uid}"),
            }))
        })
        .build()?;

    let options = ReadOptions::default();
    let first = cache.get(&10001, &options).await?;
    let second = cache.get(&10001, &options).await?;
    println!("first={first:?}, second={second:?}");

    let batch = cache.mget(&[10001, 10002, 10003], &options).await?;
    println!("batch={batch:?}");

    cache.del(&10001).await?;
    Ok(())
}
