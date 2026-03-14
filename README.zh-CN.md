# accelerator - Rust 多级缓存运行时

[English](README.md) | [简体中文](README.zh-CN.md)

`accelerator` 是一个可插拔、异步优先的 Rust 多级缓存组件，面向高并发服务场景。
默认后端为本地缓存（L1: `moka`）+ 远程缓存（L2: `redis`），并支持自定义后端替换。

## 🚀 核心能力

- 多级模式：`Local`、`Remote`、`Both`
- 统一 API：`get`、`mget`、`set`、`mset`、`del`、`mdel`、`warmup`
- 回源协议：`Loader<K, V>` + `MLoader<K, V>`
- 稳定性策略：空值缓存、TTL 抖动、提前刷新、陈旧值回退
- 一致性策略：Redis Pub/Sub 失效广播
- 可观测性：`metrics_snapshot`、`diagnostic_snapshot`、`otel_metric_points`
- 宏支持：`cacheable`、`cacheable_batch`、`cache_put`、`cache_evict`、`cache_evict_batch`

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

## ⚡ 快速开始

```bash
cargo run --example fixed_backend_best_practice
```

如果 Redis 不可用，可先运行本地模式示例；详细说明见：

- [`docs/local-stack-integration.md`](docs/local-stack-integration.md)
- [`docs/zh/local-stack-integration.zh-CN.md`](docs/zh/local-stack-integration.zh-CN.md)
