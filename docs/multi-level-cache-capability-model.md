# 多级缓存抽象能力模型（Rust，参考 JetCache）

## 1. 文档目标

本文档用于定义一个可扩展、可演进的 Rust 多级缓存组件的抽象能力模型，作为后续接口设计、实现拆分、测试验证与运维治理的统一基线。  
设计参考 JetCache 的能力边界，但以 Rust 生态与异步并发模型为核心约束。

---

## 2. 设计原则

1. **先抽象后实现**：先定义能力模型与契约，再选择具体库（如 `moka`、`redis`）。
2. **固定后端优先**：本项目落地版默认固定 `moka`（L1）+ `redis`（L2），优先保证主路径可读性与稳定性。
3. **Rust 约束明确**：默认 Rust 2024 Edition；实现层禁止 `unsafe`（除非经过 ADR 单独评审）。
4. **默认安全一致**：默认策略优先保证数据正确性与可解释性，再优化极限性能。
5. **异步友好**：运行时 API 直接使用原生 `async fn`，Loader 接口使用原生 `async fn in trait`，不使用 `async_trait`。
6. **可观测先行**：每个关键路径都必须有指标、日志、追踪字段。
7. **渐进式能力增强**：MVP 可先覆盖读写/回源，后续再叠加刷新、广播失效、注解宏。
8. **静态分发优先**：运行时不依赖 `dyn` 后端对象；通过泛型在编译期完成分发。

### 2.1 当前实现落地约束（MVP）

1. 对外主类型为 `LevelCache<K, V, LD>`，后端固定为 `moka`（L1）+ `redis`（L2）。
2. Builder 主入口为 `LevelCacheBuilder<K, V>`，不再提供后端替换入口。
3. `get/mget` 默认 miss 自动回源；仅在 `ReadOptions.disable_load=true` 或未配置 loader 时跳过回源。
4. `mget` 返回 `HashMap<K, Option<V>>`，保证覆盖全部请求 key。
5. singleflight 固定使用 `singleflight-async`，不再引入 provider 抽象层。
6. `ttl_jitter_ratio` 已落地（默认 `0.1`），用于错峰过期，降低雪崩风险。
7. L1 淘汰行为固定为 `moka` 原生策略，不暴露 LRU/LFU/WTinyLFU 切换配置。
8. 内置基础观测：`LevelCache::metrics_snapshot()` + `tracing` span（`cache.get/mget/set/del/load`）。

---

## 3. 系统边界

### 3.1 In Scope（本组件负责）

- 单进程本地缓存（Local）与分布式缓存（Remote）的统一访问编排。
- 统一缓存 API：`get` / `set` / `del` / `mget` / `mset` / `mdel`
- 缓存穿透、击穿、雪崩等常见风险的治理策略。
- 一致性策略抽象（读路径、写路径、失效策略）。
- 指标、日志、追踪、错误分类。

### 3.2 Out of Scope（本组件不直接负责）

- 业务数据库事务语义。
- 跨地域多活数据复制冲突解决。
- 强一致分布式锁系统（仅支持轻量回源合并）。
- 特定框架（Axum/Actix/Tonic）耦合逻辑。

---

## 4. 核心概念模型

### 4.1 分层模型

- **L1（Local）**：进程内缓存，低延迟，高命中，容量受限。
- **L2（Remote）**：跨实例共享缓存，容量大，延迟相对更高。
- **Loader（回源器）**：缓存 miss 时的数据加载函数。
- **Invalidation Channel（失效通道）**：用于跨实例同步 L1 失效。

### 4.2 数据实体

- **CacheKey**：标准化后的缓存键（`area + business_key`）。
- **CacheEntry<V>**：缓存值与元数据封装。
- **TTL Policy**：过期策略（固定 TTL、随机抖动、逻辑过期）。
- **Null Value Marker**：空值缓存标记，避免缓存穿透。
- **Value Semantics**：值对象的读写语义（copy-on-read / copy-on-write / shared-arc）。

### 4.3 生命周期状态

1. `Absent`：不存在。
2. `HitFresh`：命中且新鲜。
3. `HitStale`：命中但逻辑过期（可返回旧值并触发刷新）。
4. `Loading`：并发回源合并中。
5. `Invalidated`：显式失效，等待下一次加载。

---

## 5. 抽象接口模型

本节给出“能力边界 + 契约语义 + 推荐类型结构”。  
本版本是**彻底固定后端**实现：运行时固定 `moka` + `redis`，对外暴露最终 API，不再暴露可替换后端 trait。

### 5.1 接口分层与职责

1. **Storage 层（固定实现）**  
   - `MokaBackend<V>`：本地缓存，提供 `get/mget/set/mset/del/mdel`。
   - `RedisBackend<V>`：远端缓存，提供 `get/mget/set/mset/del/mdel`。
2. **Orchestration 层（统一编排）**  
   - `LevelCache<K, V, LD>`：统一处理 L1/L2 路由、回填、回源、singleflight。
3. **Loader 层（可插拔回源）**  
   - `Loader<K, V>` / `MLoader<K, V>`：仅负责 miss 回源。

### 5.2 基础类型契约

```rust
use std::time::Duration;

pub type CacheResult<T> = Result<T, CacheError>;

#[derive(Clone, Debug)]
pub enum StoredValue<V> {
    Value(V),
    Null,
}

#[derive(Clone, Debug)]
pub struct StoredEntry<V> {
    pub value: StoredValue<V>,
    pub expire_at: std::time::Instant,
}

#[derive(Clone, Debug)]
pub struct ReadOptions {
    pub allow_stale: bool,
    pub disable_load: bool,
}

#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub local_ttl: Duration,
    pub remote_ttl: Duration,
    pub null_ttl: Duration,
    pub ttl_jitter_ratio: Option<f64>,
    pub cache_null_value: bool,
    pub penetration_protect: bool,
    pub loader_timeout: Option<Duration>,
    pub copy_on_read: bool,
    pub copy_on_write: bool,
    pub read_value_mode: ReadValueMode,
}
```

约束说明：

- `StoredEntry` 必须保存值 + 过期元数据，不允许仅缓存裸值。
- `StoredValue::Null` 表示“明确缓存空结果”，和 miss 语义不同。
- `ReadOptions.disable_load=true` 时，`get/mget` 必须严格 cache-only。

### 5.3 值所有权与复制语义（copy-on-read / copy-on-write）

> 结论：Rust 仍然需要显式建模 copy 语义。  
> `Send + Sync` 解决的是并发安全，不等于业务快照隔离。

语义约束：

1. **copy-on-read=true（默认）**：返回值与缓存内部状态隔离。
2. **copy-on-write=true（默认）**：写入时获取独立所有权，避免外部后续修改影响缓存。
3. **SharedArc 模式（可选）**：用于只读大对象优化；若值包含 interior mutability，需由业务自行保证语义正确。

### 5.4 固定后端接口（不再抽象 trait）

```rust
pub struct MokaBackend<V> { /* ... */ }

impl<V> MokaBackend<V> {
    pub async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>>;
    pub async fn mget(&self, keys: &[String]) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>>;
    pub async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()>;
    pub async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()>;
    pub async fn del(&self, key: &str) -> CacheResult<()>;
    pub async fn mdel(&self, keys: &[String]) -> CacheResult<()>;
}

pub struct RedisBackend<V> { /* ... */ }

impl<V> RedisBackend<V> {
    pub async fn get(&self, key: &str) -> CacheResult<Option<StoredEntry<V>>>;
    pub async fn mget(&self, keys: &[String]) -> CacheResult<HashMap<String, Option<StoredEntry<V>>>>;
    pub async fn set(&self, key: &str, entry: StoredEntry<V>) -> CacheResult<()>;
    pub async fn mset(&self, entries: HashMap<String, StoredEntry<V>>) -> CacheResult<()>;
    pub async fn del(&self, key: &str) -> CacheResult<()>;
    pub async fn mdel(&self, keys: &[String]) -> CacheResult<()>;
}
```

语义要求：

- `mget` 返回必须覆盖全部请求 key（miss 也要返回 `key -> None`）。
- `del/mdel` 必须幂等。
- Redis 批量接口要求：`mget` 使用原生 `MGET`，`mset` 使用 pipeline 优化。

### 5.5 回源与编排接口

Loader 接口改为**原生 `async fn trait`**（不使用 `async_trait`）：

```rust
#[allow(async_fn_in_trait)]
pub trait Loader<K, V>: Send + Sync {
    async fn load(&self, key: &K) -> CacheResult<Option<V>>;
}

#[allow(async_fn_in_trait)]
pub trait MLoader<K, V>: Loader<K, V>
where
    K: Eq + Hash + Clone,
{
    async fn mload(&self, keys: &[K]) -> CacheResult<HashMap<K, Option<V>>>;
}
```

编排层对外只暴露最终结构体：

```rust
pub struct LevelCache<K, V, LD = NoopLoader> { /* ... */ }

impl<K, V, LD> LevelCache<K, V, LD> {
    pub async fn get(&self, key: &K, opts: &ReadOptions) -> CacheResult<Option<V>>;
    pub async fn mget(&self, keys: &[K], opts: &ReadOptions) -> CacheResult<HashMap<K, Option<V>>>;
    pub async fn set(&self, key: &K, value: Option<V>) -> CacheResult<()>;
    pub async fn mset(&self, entries: HashMap<K, Option<V>>) -> CacheResult<()>;
    pub async fn del(&self, key: &K) -> CacheResult<()>;
    pub async fn mdel(&self, keys: &[K]) -> CacheResult<()>;
    pub fn metrics_snapshot(&self) -> CacheMetricsSnapshot;
}
```

Builder：

```rust
pub struct LevelCacheBuilder<K, V, LD = NoopLoader> { /* ... */ }
```

行为约束：

1. `get/mget` 默认 miss 自动回源（除非 `disable_load=true` 或未配置 loader）。
2. `mget` 默认返回 `HashMap<K, Option<V>>`，并覆盖所有请求 key。
3. 回源成功后按 mode 写回（`Local`/`Remote`/`Both`）。
4. 空值在 `cache_null_value=true` 时按 `null_ttl` 写入。

### 5.5.1 singleflight 实现约定（singleflight-async）

为减少重复造轮子，统一使用 [singleflight-async](https://crates.io/crates/singleflight-async) 实现“同 key 并发回源合并”（不提供 provider 可配置项）。

实现约束：

1. 合并粒度必须是标准化后的 `encoded_key`，保证跨 `get/mget` 行为一致。
2. 仅在 miss 且需要回源时进入 singleflight；命中路径不得经过 singleflight。
3. singleflight 仅负责去重，不替代超时、熔断、降级策略。

### 5.6 扩展接口（规划项）

当前版本不对外开放“后端插件”接口；扩展方向主要是：

1. 失效广播（Pub/Sub / Stream）。
2. Codec 插件（json/bincode/custom）。
3. 观测插件（metrics/tracing hooks）。

### 5.7 错误模型（当前实现）

```rust
pub enum CacheError {
    InvalidConfig(String),
    Backend(String),
    Loader(String),
    Timeout(&'static str),
}
```

要求：

- 错误要能映射到指标标签（`kind`/`op`）。
- 保留可读错误上下文（后端操作名、超时来源）。

### 5.8 Async 选型约定（固定版）

1. 对外核心 API（`LevelCache`、`MokaBackend`、`RedisBackend`）使用原生 `async fn`。
2. Loader trait 使用原生 `async fn in trait`，并显式 `#[allow(async_fn_in_trait)]`。
3. 不使用 `async_trait`，不引入动态分发包装。

### 5.9 接口语义总表

| 接口 | 入参重点 | 返回语义 | 必须保证 |
| --- | --- | --- | --- |
| `get`（Local/Redis） | key | `None`=miss，`Some(entry)`=命中 | 不返回过期数据 |
| `mget`（Local/Redis） | 批量 key | `HashMap<String, Option<StoredEntry<V>>>` | 不漏 key |
| `set/mset` | 值 + TTL 元数据 | `Ok(())`=写入生效 | 失败不可吞 |
| `del/mdel` | key / keys | 幂等删除 | key 不存在不报错 |
| `get`（编排层） | key + `ReadOptions` | miss 且可回源时自动 load | 同 key 并发只回源一次 |
| `mget`（编排层） | 批量 key + `ReadOptions` | `HashMap<K, Option<V>>` | 覆盖全部请求 key |

---

## 6. 能力分层（Capability Matrix）

### L0：基础能力（MVP 必须）

1. `LOCAL / REMOTE / BOTH` 三种缓存模式。
2. 基本 API：`get`、`set`、`del`、`mget`、`mset`、`mdel`。
3. L1 miss 后查 L2，L2 命中回填 L1。
4. 缓存过期控制（TTL）。
5. 空值缓存（可配置开关与 TTL）。
6. `get/mget` 在配置 loader 时默认 miss 自动回源（可通过 `ReadOptions.disable_load` 关闭）。
7. 值语义配置（copy-on-read/copy-on-write/shared-arc）。
8. L1 由 `moka` 固定实现与固定淘汰行为，不对外开放策略切换。

### L1：高并发稳定性（MVP 强烈建议）

1. **单 key 回源合并（singleflight）**，抑制击穿。
2. **TTL 抖动（jitter）**，抑制同刻过期雪崩。
3. 回源超时与熔断保护（至少超时）。
4. 失败降级策略（返回旧值/失败直出可配置）。

### L2：一致性增强（二期）

1. 写后失效广播（Pub/Sub 或 Stream）。
2. 收到失效事件后清理 L1。
3. 逻辑过期 + 后台刷新（refresh ahead）。
4. 并发更新版本控制（可选）。

### L3：易用性增强（三期）

1. 注解/宏风格 API（`#[cached]`）。
2. 批量读取/写入能力（mget/mset）。
3. 命名空间治理（area 配额、按业务隔离）。
4. 管理接口（统计、热 key、强制失效）。

---

## 7. 关键流程模型

### 7.1 读路径（BOTH 模式）

1. 查询 L1，命中且未过期：直接返回。
2. L1 miss：查询 L2。
3. L2 命中：写回 L1（短 TTL）并返回。
4. L2 miss 且有 loader：进入 singleflight 回源。
5. 回源成功：写 L2 + 写 L1 后返回。
6. 回源为空：按 null-cache 策略写入短 TTL 空值后返回。
7. L2 miss 且无 loader：直接返回 miss。

### 7.2 写路径（推荐“数据库优先 + 删缓存”）

1. 业务先写 DB（由业务层负责事务）。
2. 组件执行 `del(L2)`。
3. 组件执行 `del(L1)`。
4. 发布失效事件到通道（通知其他实例清理 L1）。

> 说明：默认不建议“先写缓存再写 DB”，以减少多节点不一致窗口。

### 7.3 删除路径

1. 本实例执行 `del(L2)` + `del(L1)`。
2. 广播失效消息。
3. 其他实例收到后删除各自 L1。

---

## 8. 一致性策略模型

### 8.1 一致性级别定义（面向业务可选）

- **EventuallyConsistent（默认）**：允许短时间旧值，强调性能。
- **BoundedStaleness**：限定陈旧窗口（例如逻辑过期 + 背景刷新）。
- **ReadThroughStrict（高成本）**：关键路径尽量直达 L2/源，牺牲延迟。

### 8.2 冲突与并发更新

- 可选 `version` 字段支持 compare-and-set 语义。
- 写入顺序优先依据“源数据更新时间”，避免旧写覆盖新写。
- 多实例场景下通过“失效优先于回填”降低脏值驻留时间。

---

## 9. 配置模型

配置能力分为“当前已落地”与“后续规划”两类：

当前已落地：
- `cache_type`: `Local | Remote | Both`
- `area`: 业务命名空间
- `local_ttl`, `remote_ttl`, `null_ttl`
- `ttl_jitter_ratio`（当前已实现，默认 `Some(0.1)`，范围 `[0.0, 1.0]`）
- `copy_on_read`, `copy_on_write`
- `read_value_mode`: `OwnedClone | SharedArc`
- `local_max_capacity`（通过 `local::moka::<V>().max_capacity(...)` 配置）
- `penetration_protect`: bool
- `loader_timeout_ms`

后续规划（当前未落地）：
- `refresh_ahead`: bool
- `refresh_before_expire_ms`
- `allow_stale_on_error`
- `codec`: `Json | Bincode | Custom`

固定约束：

- singleflight 实现统一绑定 `singleflight-async`，不做 provider 抽象。
- L1 使用 `moka`，淘汰策略由 `moka` 内部实现决定；组件不再暴露 `local_eviction_policy`。

要求：

- 参数有合理默认值。
- 启动时做配置校验并给出明确错误。
- 运行时可暴露生效配置快照（便于排障）。

### 9.1 Copy 语义默认值建议

建议默认配置：

- `copy_on_read=true`
- `copy_on_write=true`
- `read_value_mode=OwnedClone`

当业务明确是“大对象只读模型”时，可切换 `read_value_mode=SharedArc` 以减少拷贝开销，但必须满足：

1. 值对象逻辑不可变（或等价不可变）。
2. 不将内部 `Mutex/RwLock/RefCell` 暴露给上层做写操作。
3. 文档中声明“返回的是共享视图，不保证业务快照隔离”。

### 9.2 缓存淘汰策略（固定语义）

当前版本遵循“彻底固定后端”原则：

- L1 固定为 `moka`，本组件只暴露容量相关配置（如 `max_capacity`），不提供淘汰策略枚举切换。
- L2 固定为 Redis，淘汰行为由 Redis 服务端 `maxmemory-policy` 决定。
- 组件侧只保证 TTL 与读写路径语义，不对 Redis 全局淘汰策略做运行时接管。

工程建议：

1. 将 Redis `maxmemory-policy` 纳入环境基线（IaC/启动脚本）统一治理。
2. 在压测中关注命中率与抖动，不再把“切换 L1 算法”作为调优手段。
3. 若未来确实需要“可切换算法”，通过引入新 L1 实现完成，而不是给 `moka` 透传伪配置。

---

## 10. 可观测性模型

### 10.1 指标（Metrics）

当前版本通过 `LevelCache::metrics_snapshot()` 暴露基础计数（内存快照）：

- `local_hit` / `local_miss`
- `remote_hit` / `remote_miss`
- `load_total` / `load_success` / `load_timeout` / `load_error`

后续可再对接 Prometheus/OpenTelemetry exporter。

### 10.2 日志（Logging）

关键字段：`area`、`key_hash`、`layer`、`result`、`latency_ms`、`trace_id`。  
注意不要输出原始敏感 key/value。

### 10.3 追踪（Tracing）

当前版本已通过 `tracing` 打点以下 span：

- `cache.get`
- `cache.mget`
- `cache.set`
- `cache.mset`
- `cache.del`
- `cache.mdel`
- `cache.load`

需要业务侧注入 subscriber（例如 `tracing-subscriber`）后可见。

---

## 11. 扩展点模型

1. **存储后端扩展（后续）**：当前版本固定 `moka+redis`，若未来需要扩展到 Memcached/Etcd，将新增单独适配层而非直接开放通用后端 trait。
2. **编码扩展**：可插拔 `Codec`，支持性能与可读性取舍。
3. **键策略扩展**：不同业务可实现自定义 `KeyConverter`。
4. **失效通道扩展**：Pub/Sub、消息队列、CDC 事件。
5. **策略扩展**：热 key 保护、分级 TTL、业务权重限流。

---

## 12. 可靠性与测试模型

### 12.1 测试分层

- 单元测试：键构造、TTL 判断、空值缓存逻辑、copy-on-read/copy-on-write 行为。
- 组件测试：L1/L2 编排、回填、singleflight 行为。
- 集成测试：对接真实 Redis，验证超时与网络抖动（`tests/redis_integration.rs`；Redis 不可用时自动跳过）。
- 压测：热点 key 并发、批量 miss、命中率与延迟抖动、故障注入恢复。

### 12.2 故障注入场景

1. L2 超时/不可达。
2. Loader 慢查询/错误。
3. 广播通道消息丢失或延迟。
4. 大规模 key 同时过期。

验收目标：在故障场景下组件行为可预测，且指标能反映根因。

---

## 13. 安全与治理模型

- key 规范化，避免注入式拼接风险。
- value 序列化大小限制，防止超大对象进入缓存。
- 敏感数据默认不缓存（或加密后缓存）。
- 支持按 `area` 做隔离与限额策略。
- 管理接口需鉴权（如果后续提供管理 API）。

---

## 14. 里程碑建议

### Milestone 1（MVP）

- [x] `get/set/del/mget/mset/mdel`
- [x] L1 + L2 编排
- [x] singleflight + null-cache + ttl-jitter
- [x] metrics/tracing 基础指标

### Milestone 2（增强）

- Warm up 预热机制
- 失效广播
- refresh ahead
- 错误分级降级策略

### Milestone 3（体验）

- 宏注解 API
- 批量接口
- 治理/观测控制面

---

## 15. 术语表

- **L1**：本地进程缓存层。
- **L2**：远端共享缓存层。
- **回源（Load）**：缓存 miss 后从数据源拉取数据。
- **Singleflight**：同 key 并发请求只触发一次回源。
- **Null Cache**：将空结果短期缓存以减少穿透。
- **Refresh Ahead**：在过期前提前异步刷新缓存。

---

## 16. Crate 对外使用模型（固定后端）

本节补充“作为一个对外 crate，业务方如何真正接入”。

### 16.1 对外 API 形态

当前版本只暴露固定后端运行时能力，不暴露后端抽象扩展点：

- `LevelCacheBuilder<K, V>`：构建入口。
- `LevelCache<K, V, LD>`：最终缓存运行时。
- `local::moka::<V>()`：L1 构建器。
- `remote::redis::<V>()`：L2 构建器。
- `Loader` / `MLoader` / `FnLoader`：回源能力。
- `CacheMetricsSnapshot`：运行时指标快照。

导入方式示例：

```rust
use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::{CacheMode, ReadValueMode};
```

### 16.2 Builder 最佳实践（固定后端）

仓库中可直接运行的示例：`examples/fixed_backend_best_practice.rs`。

运行方式：

```bash
cargo run --example fixed_backend_best_practice
```

示例要点：

1. 显式配置 `mode=Both`，并传入 `moka + redis`。
2. 开启 `penetration_protect`（singleflight-async）与 `loader_timeout`。
3. 使用 `loader_fn` 定义最小回源路径。
4. 通过 `ReadOptions.disable_load` 控制是否允许自动回源。

### 16.3 Redis 集成测试策略

当前版本已提供真实 Redis 集成测试：`tests/redis_integration.rs`。

测试约束：

1. 每个用例运行前先探测 Redis 可达性（`PING`）。
2. Redis 不可达时自动跳过该用例，不影响本地单测结果。
3. 默认地址为 `redis://127.0.0.1:6379`，可通过 `ACCELERATOR_TEST_REDIS_URL` 覆盖。

推荐执行方式：

```bash
cargo test
```

若本地 Redis 已启动，将同时执行单元测试与 Redis 集成测试。

### 16.4 当前版本边界说明

- 过程宏（如 `#[cached]`）尚未落地，不属于当前 MVP 对外承诺。
- 若后续需要过程宏，会以“对 `LevelCache` 的语法糖包装”方式引入，不改变固定后端主路径。
