# accelerator - Rust 多级缓存运行时

[English](README.md) | [简体中文](README.zh-CN.md)

Accelerator 是一个可插拔、异步优先的 Rust 多级缓存运行时，面向高并发服务场景。
它提供统一的本地缓存（L1）+ 远程缓存（L2）访问 API，内置回源加载（Miss load）、批量加载、失效广播与可观测能力。

## 🚀 核心能力

- 🧭 多级模式：`Local`、`Remote`、`Both`
- 🧱 默认后端：`moka`（L1）+ `redis`（L2）
- 🔌 可替换后端 trait：
  - `LocalBackend<V>`
  - `RemoteBackend<V>`
  - `InvalidationSubscriber`
- 🍱 统一运行时 API：
  - `get`、`mget`、`set`、`mset`、`del`、`mdel`、`warmup`
- 📥 回源协议：
  - 单 key：`Loader<K, V>`
  - 批量：`MLoader<K, V>`
- 🛡️ miss 处理：
  - 单 key miss 可启用 singleflight 去重（`penetration_protect`）
  - 批量 miss（`mget`）直接走 `MLoader::mload`
- 🔄 稳定性能力：
  - 空值缓存（`cache_null_value`、`null_ttl`）
  - TTL 抖动（`ttl_jitter_ratio`）
  - 提前刷新（`refresh_ahead`）
  - 陈旧值回退（`stale_on_error`）
- 📡 跨实例本地缓存一致性：
  - Redis Pub/Sub 失效广播
- 👀 可观测性：
  - 运行时计数（`metrics_snapshot`）
  - 诊断快照（`diagnostic_snapshot`）
  - OTel 友好指标点（`otel_metric_points`）
  - 核心链路 tracing spans
- 🪄 过程宏：
  - `cacheable`、`cacheable_batch`、`cache_put`、`cache_evict`、`cache_evict_batch`

## 📋 目录

- [📦 安装](#-安装)
- [🤠 快速开始](#-快速开始)
- [🍱 API 概览](#-api-概览)
- [🧩 宏使用说明](#-宏使用说明)
- [🏗️ 后端扩展](#️-后端扩展)
- [🪄 示例](#-示例)
- [🏎️ Benchmark 与回归门禁](#️-benchmark-与回归门禁)
- [🧪 集成测试](#-集成测试)
- [🧰 本地全栈联调](#-本地全栈联调)
- [📚 文档导航](#-文档导航)

## 📦 安装

推荐直接从 crates.io 引入：

```toml
[dependencies]
accelerator = "0.1.0"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

## 🤠 快速开始

### 仅本地缓存（L1 = moka）

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

### 双层缓存（L1 + L2）

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

## 🍱 API 概览

核心运行时：

- `LevelCache<K, V, LD, LB, RB>`
- `ReadOptions { allow_stale, disable_load }`
- `CacheMode::{Local, Remote, Both}`

主要方法：

- 读取：`get`、`mget`
- 写入：`set`、`mset`
- 失效：`del`、`mdel`
- 预热：`warmup`

诊断与指标：

- `metrics_snapshot() -> CacheMetricsSnapshot`
- `diagnostic_snapshot() -> CacheDiagnosticSnapshot`
- `otel_metric_points() -> Vec<OtelMetricPoint>`

回源 trait：

- `Loader<K, V>::load(&K) -> Future<CacheResult<Option<V>>>`
- `MLoader<K, V>::mload(&[K]) -> Future<CacheResult<HashMap<K, Option<V>>>>`

## 🧩 宏使用说明

宏统一从以下模块导入：

```rust
use accelerator::macros::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};
```

宏语义与约束：

- `#[cacheable(...)]`：先读缓存，miss 执行函数体，成功后 `set` 回写。
- `#[cacheable_batch(...)]`：先 `mget`，仅对 misses 执行业务，再 `mset` 回写。
- `#[cache_put(...)]`：先执行业务逻辑，成功后写缓存。
- `#[cache_evict(...)]` / `#[cache_evict_batch(...)]`：默认在函数成功后失效（`before = false`）。
- 仅支持 `impl` 块中的 `async fn` 方法（第一个参数必须是 `&self` 或 `&mut self`）。
- `on_cache_error` 支持 `"ignore"`（默认）和 `"propagate"`。

单 key 示例：

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

批量示例：

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

可运行参考：

- `examples/macro_best_practice.rs`
- `examples/macro_batch_best_practice.rs`

## 🏗️ 后端扩展

如需替换默认后端：

1. 为本地缓存实现 `LocalBackend<V>`。
2. 为远程缓存实现 `RemoteBackend<V>` 与 `InvalidationSubscriber`。
3. 通过 `LevelCacheBuilder::local(...)` 与 `LevelCacheBuilder::remote(...)` 注入。

运行时采用泛型静态分发，不依赖运行时 `dyn` 对象。

## 🪄 示例

查看 `examples/`：

- `fixed_backend_best_practice.rs`（moka + redis）
- `macro_best_practice.rs`（单 key 宏流程）
- `macro_batch_best_practice.rs`（批量宏流程）
- `clickstack_otlp.rs`（可选 OTLP 启动，feature `otlp`）

运行：

```bash
cargo run --example fixed_backend_best_practice
```

如果 `ACCELERATOR_REDIS_URL`（默认 `redis://127.0.0.1:6379`）不可达，示例会优雅退出。

## 🏎️ Benchmark 与回归门禁

一键脚本：

```bash
./scripts/bench.sh
./scripts/bench.sh bench-local --runs 3 --sample-size 60
./scripts/bench.sh bench-redis --runs 3 --sample-size 60 --redis-url redis://127.0.0.1:6379
./scripts/bench.sh regression --threshold 0.15
```

原生命令：

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 cargo bench --bench cache_path_bench -- --sample-size=60
cargo run --bin export_bench_baseline --
cargo run --bin check_bench_regression -- --threshold 0.15
```

详细流程见：`docs/performance-engineering-playbook.md`

## 🧪 集成测试

Redis 集成测试位于 `tests/redis_integration.rs`。

- 通过 `cargo test` 运行。
- Redis 不可用时，相关测试会按设计跳过。
- 可通过 `ACCELERATOR_TEST_REDIS_URL` 覆盖端点。

## 🧰 本地全栈联调

启动本地栈：

```bash
cd scripts
docker compose up -d
```

运行端到端测试：

```bash
cargo test --test redis_integration
cargo test --test stack_integration
```

`stack_integration` 使用真实 `sqlx + Postgres` 回源链路。

ClickStack UI：`http://127.0.0.1:8080`  
OTLP 端口：`4317` 与 `4318`

## 📚 文档导航

英文文档为默认版本；中文文档位于 `docs/zh/`。

| 主题 | English | 中文（简体） |
| --- | --- | --- |
| README | [`README.md`](README.md) | [`README.zh-CN.md`](README.zh-CN.md) |
| 术语基线 | [`docs/terminology.md`](docs/terminology.md) | [`docs/zh/terminology.zh-CN.md`](docs/zh/terminology.zh-CN.md) |
| 能力模型 | [`docs/multi-level-cache-capability-model.md`](docs/multi-level-cache-capability-model.md) | [`docs/zh/multi-level-cache-capability-model.zh-CN.md`](docs/zh/multi-level-cache-capability-model.zh-CN.md) |
| 性能手册 | [`docs/performance-engineering-playbook.md`](docs/performance-engineering-playbook.md) | [`docs/zh/performance-engineering-playbook.zh-CN.md`](docs/zh/performance-engineering-playbook.zh-CN.md) |
| 运维手册 | [`docs/cache-ops-runbook.md`](docs/cache-ops-runbook.md) | [`docs/zh/cache-ops-runbook.zh-CN.md`](docs/zh/cache-ops-runbook.zh-CN.md) |
| 本地联调 | [`docs/local-stack-integration.md`](docs/local-stack-integration.md) | [`docs/zh/local-stack-integration.zh-CN.md`](docs/zh/local-stack-integration.zh-CN.md) |
| 扁平化规范 | [`docs/code-flattening-guideline.md`](docs/code-flattening-guideline.md) | [`docs/zh/code-flattening-guideline.zh-CN.md`](docs/zh/code-flattening-guideline.zh-CN.md) |
