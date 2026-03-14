# accelerator - Multi-Level Cache Runtime for Rust

[English](README.md) | [简体中文](README.zh-CN.md)

Accelerator is a pluggable, async-first cache runtime for high-concurrency Rust services.
It provides a unified API over local cache (L1) and remote cache (L2), with miss load
(source-of-truth load), batch loading, invalidation broadcast, and built-in observability.

## 🚀 Features

- 🧭 Multi-level modes: `Local`, `Remote`, `Both`
- 🧱 Default backends: `moka` (L1) + `redis` (L2)
- 🔌 Pluggable backends via traits:
  - `LocalBackend<V>`
  - `RemoteBackend<V>`
  - `InvalidationSubscriber`
- 🍱 Unified runtime APIs:
  - `get`, `mget`, `set`, `mset`, `del`, `mdel`, `warmup`
- 📥 Loader contracts:
  - single-key: `Loader<K, V>`
  - batch: `MLoader<K, V>`
- 🛡️ Miss handling:
  - single-key miss can use singleflight dedup (`penetration_protect`)
  - batch miss (`mget`) uses `MLoader::mload` directly
- 🔄 Resilience and stability:
  - negative cache (`cache_null_value`, `null_ttl`)
  - TTL jitter (`ttl_jitter_ratio`)
  - refresh-ahead (`refresh_ahead`)
  - stale fallback (`stale_on_error`)
- 📡 Cross-instance local cache consistency:
  - Redis Pub/Sub invalidation broadcast
- 👀 Observability:
  - runtime counters (`metrics_snapshot`)
  - diagnostic state (`diagnostic_snapshot`)
  - OTel-friendly metric points (`otel_metric_points`)
  - tracing spans on core paths
- 🪄 Procedural macros:
  - `cacheable`, `cacheable_batch`, `cache_put`, `cache_evict`, `cache_evict_batch`

## 📋 Table of Contents

- [📦 Installation](#installation)
- [🤠 Quick Start](#quick-start)
- [🍱 API Overview](#api-overview)
- [🧩 Macro Usage](#macro-usage)
- [🏗️ Backend Extension](#backend-extension)
- [🪄 Examples](#examples)
- [🏎️ Benchmark and Regression Gate](#benchmark-and-regression-gate)
- [🧪 Integration Tests](#integration-tests)
- [🧰 Local Full Stack](#local-full-stack)
- [📚 Documentation](#documentation)

## 📦 Installation

Use as a local workspace crate:

```toml
[dependencies]
accelerator = { path = "/path/to/accelerator" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

If you publish/use from registry, replace with the registry version.

## 🤠 Quick Start

### Local-only cache (L1 = moka)

```rust
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::config::CacheMode;
use accelerator::{ReadOptions, local};

#[tokio::main]
async fn main() -> accelerator::CacheResult<()> {
    let local_backend = local::moka::<String>().max_capacity(100_000).build()?;

    let cache = LevelCacheBuilder::<u64, String>::new()
        .area("user")
        .mode(CacheMode::Local)
        .local(local_backend)
        .local_ttl(Duration::from_secs(60))
        .null_ttl(Duration::from_secs(10))
        .loader_fn(|id: u64| async move { Ok(Some(format!("user-{id}"))) })
        .build()?;

    let v = cache.get(&42, &ReadOptions::default()).await?;
    assert_eq!(v, Some("user-42".to_string()));
    Ok(())
}
```

### Two-level cache (L1 + L2)

```rust
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::config::CacheMode;
use accelerator::{local, remote};

let local_backend = local::moka::<String>().max_capacity(100_000).build()?;
let remote_backend = remote::redis::<String>()
    .url("redis://127.0.0.1:6379")
    .key_prefix("demo")
    .build()?;

let cache = LevelCacheBuilder::<u64, String>::new()
    .area("user")
    .mode(CacheMode::Both)
    .local(local_backend)
    .remote(remote_backend)
    .local_ttl(Duration::from_secs(60))
    .remote_ttl(Duration::from_secs(300))
    .broadcast_invalidation(true)
    .build()?;
```

## 🍱 API Overview

Core runtime:

- `LevelCache<K, V, LD, LB, RB>`
- `ReadOptions { allow_stale, disable_load }`
- `CacheMode::{Local, Remote, Both}`

Main methods:

- Read: `get`, `mget`
- Write: `set`, `mset`
- Invalidate: `del`, `mdel`
- Warmup: `warmup`

Diagnostics and metrics:

- `metrics_snapshot() -> CacheMetricsSnapshot`
- `diagnostic_snapshot() -> CacheDiagnosticSnapshot`
- `otel_metric_points() -> Vec<OtelMetricPoint>`

Loader traits:

- `Loader<K, V>::load(&K) -> Future<CacheResult<Option<V>>>`
- `MLoader<K, V>::mload(&[K]) -> Future<CacheResult<HashMap<K, Option<V>>>>`

## 🧩 Macro Usage

Import macros from:

```rust
use accelerator::macros::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};
```

Macro behavior and constraints:

- `#[cacheable(...)]`: cache-first read, miss executes function body, then `set` write-back.
- `#[cacheable_batch(...)]`: `mget` first, loads misses only, then `mset` write-back.
- `#[cache_put(...)]`: executes function first, then `set` to cache on success.
- `#[cache_evict(...)]` / `#[cache_evict_batch(...)]`: invalidates after success by default (`before = false`).
- Macros only support `async fn` methods on `impl` blocks (`&self` / `&mut self` receiver).
- `on_cache_error` supports `"ignore"` (default) or `"propagate"`.

Minimal single-key example:

```rust
use accelerator::macros::{cache_evict, cache_put, cacheable};

impl UserService {
    #[cacheable(cache = self.cache, key = user_id, cache_none = true, on_cache_error = "ignore")]
    async fn get_user(&self, user_id: u64) -> accelerator::CacheResult<Option<User>> {
        self.repo.find_by_id(user_id).await
    }

    #[cache_put(cache = self.cache, key = user.id, value = Some(user.clone()))]
    async fn save_user(&self, user: User) -> accelerator::CacheResult<()> {
        self.repo.upsert(user.clone()).await
    }

    #[cache_evict(cache = self.cache, key = user_id, before = false)]
    async fn delete_user(&self, user_id: u64) -> accelerator::CacheResult<()> {
        self.repo.delete(user_id).await
    }
}
```

Minimal batch example:

```rust
use accelerator::macros::{cache_evict_batch, cacheable_batch};
use std::collections::HashMap;

impl UserService {
    #[cacheable_batch(cache = self.cache, keys = user_ids)]
    async fn batch_get(&self, user_ids: Vec<u64>) -> accelerator::CacheResult<HashMap<u64, Option<User>>> {
        self.repo.batch_find(&user_ids).await
    }

    #[cache_evict_batch(cache = self.cache, keys = user_ids, before = false)]
    async fn batch_delete(&self, user_ids: Vec<u64>) -> accelerator::CacheResult<()> {
        self.repo.batch_delete(&user_ids).await
    }
}
```

Runnable references:

- `examples/macro_best_practice.rs`
- `examples/macro_batch_best_practice.rs`

## 🏗️ Backend Extension

To replace default backends:

1. Implement `LocalBackend<V>` for your local cache.
2. Implement `RemoteBackend<V>` and `InvalidationSubscriber` for your remote cache.
3. Plug them into `LevelCacheBuilder::local(...)` and `LevelCacheBuilder::remote(...)`.

The runtime uses static dispatch (generics), not runtime `dyn` objects.

## 🪄 Examples

See `examples/`:

- `fixed_backend_best_practice.rs` (moka + redis)
- `macro_best_practice.rs` (macro-based single-key flow)
- `macro_batch_best_practice.rs` (macro-based batch flow)
- `clickstack_otlp.rs` (optional OTLP bootstrap, feature `otlp`)

Run:

```bash
cargo run --example fixed_backend_best_practice
```

If Redis is unavailable at `ACCELERATOR_REDIS_URL` (default `redis://127.0.0.1:6379`),
the example exits gracefully.

## 🏎️ Benchmark and Regression Gate

One-click script:

```bash
./scripts/bench.sh
./scripts/bench.sh bench-local --runs 3 --sample-size 60
./scripts/bench.sh bench-redis --runs 3 --sample-size 60 --redis-url redis://127.0.0.1:6379
./scripts/bench.sh regression --threshold 0.15
```

Raw commands:

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 cargo bench --bench cache_path_bench -- --sample-size=60
cargo run --bin export_bench_baseline --
cargo run --bin check_bench_regression -- --threshold 0.15
```

Detailed playbook: `docs/performance-engineering-playbook.md`

## 🧪 Integration Tests

Redis integration tests are in `tests/redis_integration.rs`.

- They run with `cargo test`.
- If Redis is unavailable, tests skip gracefully where designed.
- Override endpoint with `ACCELERATOR_TEST_REDIS_URL`.

## 🧰 Local Full Stack

Start local stack:

```bash
cd scripts
docker compose up -d
```

Run end-to-end tests:

```bash
cargo test --test redis_integration
cargo test --test stack_integration
```

`stack_integration` uses real `sqlx + Postgres` loader flow.

ClickStack UI: `http://127.0.0.1:8080`  
OTLP ingest ports: `4317` and `4318`

## 📚 Documentation

English is the default documentation language. Chinese versions are maintained under `docs/zh/`.

| Topic | English | 中文（简体） |
| --- | --- | --- |
| README | [`README.md`](README.md) | [`README.zh-CN.md`](README.zh-CN.md) |
| Terminology Baseline | [`docs/terminology.md`](docs/terminology.md) | [`docs/zh/terminology.zh-CN.md`](docs/zh/terminology.zh-CN.md) |
| Capability Model | [`docs/multi-level-cache-capability-model.md`](docs/multi-level-cache-capability-model.md) | [`docs/zh/multi-level-cache-capability-model.zh-CN.md`](docs/zh/multi-level-cache-capability-model.zh-CN.md) |
| Performance Playbook | [`docs/performance-engineering-playbook.md`](docs/performance-engineering-playbook.md) | [`docs/zh/performance-engineering-playbook.zh-CN.md`](docs/zh/performance-engineering-playbook.zh-CN.md) |
| Cache Ops Runbook | [`docs/cache-ops-runbook.md`](docs/cache-ops-runbook.md) | [`docs/zh/cache-ops-runbook.zh-CN.md`](docs/zh/cache-ops-runbook.zh-CN.md) |
| Local Stack Guide | [`docs/local-stack-integration.md`](docs/local-stack-integration.md) | [`docs/zh/local-stack-integration.zh-CN.md`](docs/zh/local-stack-integration.zh-CN.md) |
| Code Flattening Guideline | [`docs/code-flattening-guideline.md`](docs/code-flattening-guideline.md) | [`docs/zh/code-flattening-guideline.zh-CN.md`](docs/zh/code-flattening-guideline.zh-CN.md) |
