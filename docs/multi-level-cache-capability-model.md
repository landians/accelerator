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
9. 已实现增强能力：`warmup`、`refresh_ahead`、`stale_on_error`、Redis Pub/Sub 失效广播。

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
    pub warmup_enabled: bool,
    pub warmup_batch_size: usize,
    pub refresh_ahead: bool,
    pub refresh_ahead_window: Duration,
    pub stale_on_error: bool,
    pub broadcast_invalidation: bool,
    pub read_value_mode: ReadValueMode,
}
```

约束说明：

- `StoredEntry` 必须保存值 + 过期元数据，不允许仅缓存裸值。
- `StoredValue::Null` 表示“明确缓存空结果”，和 miss 语义不同。
- `ReadOptions.disable_load=true` 时，`get/mget` 必须严格 cache-only。

### 5.3 值所有权与复制语义

> 结论：Rust 仍然需要显式建模 copy 语义。  
> `Send + Sync` 解决的是并发安全，不等于业务快照隔离。

语义约束：

1. **OwnedClone（当前默认）**：读路径返回拥有权值，和缓存内部状态隔离。
2. **SharedArc（预留）**：用于只读大对象优化；若值包含 interior mutability，需由业务自行保证语义正确。
3. 当前版本不再暴露 `copy_on_read/copy_on_write` 开关，避免“配置存在但主流程未生效”的误导。

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
    pub async fn warmup(&self, keys: &[K]) -> CacheResult<usize>;
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
5. `warmup_enabled=true` 时，`warmup` 才会执行预热逻辑。
6. `warmup` 会按 `warmup_batch_size` 分批执行 `mget`，避免单次请求 key 过大。
7. `refresh_ahead=true` 且命中键接近过期时，会触发回源刷新。
8. `stale_on_error=true` 且 `ReadOptions.allow_stale=true` 时，加载失败可回退本地旧值。
9. `broadcast_invalidation=true` 时，`del/mdel` 会通过 Redis Pub/Sub 广播失效并清理其他实例 L1。

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

### 7.3.1 Redis Pub/Sub 失效广播时序图（A 删除 -> B 清 L1）

```mermaid
sequenceDiagram
    participant Client as 业务请求
    participant A as 实例A(LevelCache)
    participant Redis as Redis(Pub/Sub)
    participant B as 实例B(LevelCache)
    participant BLocal as 实例B本地L1(moka)

    Note over A,B: 两个实例使用相同 area，并开启 broadcast_invalidation=true
    Note over B: B 已在读写路径触发 ensure_invalidation_listener()，后台订阅 channel=accelerator:invalidation:{area}

    Client->>A: del(key)
    A->>A: delete_remote_if_needed(encoded_key)
    A->>A: delete_local_if_needed(encoded_key)
    A->>Redis: PUBLISH InvalidationMessage{area, keys:[encoded_key]}
    Redis-->>B: 推送失效消息
    B->>B: run_invalidation_listener 解析 payload
    B->>BLocal: mdel(keys)
    B-->>Redis: 持续订阅等待下一条消息
```

补充说明：

1. 监听器不是构造时启动，而是在 `get/mget/set/mset/del/mdel/warmup` 入口懒启动（`ensure_invalidation_listener`）。
2. 广播消息体为 `InvalidationMessage { area, keys }`，频道名为 `accelerator:invalidation:{area}`。
3. 监听循环具备重试自愈能力：订阅失败/反序列化失败/本地删除失败会记录失败指标并继续重试。
4. 当前语义是“应用删除驱动的失效广播”，不包含 Redis Keyspace Notification 的自然过期事件广播。

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
- `read_value_mode`: `OwnedClone | SharedArc`
- `local_max_capacity`（通过 `local::moka::<V>().max_capacity(...)` 配置）
- `penetration_protect`: bool
- `loader_timeout_ms`
- `warmup_enabled`
- `warmup_batch_size`
- `refresh_ahead`
- `refresh_ahead_window_ms`
- `stale_on_error`
- `broadcast_invalidation`

后续规划（当前未落地）：
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
- `stale_fallback`
- `refresh_attempts` / `refresh_success` / `refresh_failures`
- `invalidation_publish` / `invalidation_publish_failures`
- `invalidation_receive` / `invalidation_receive_failures`

同时提供两种开箱导出适配：

1. `LevelCache::prometheus_metrics()`：直接导出 Prometheus text 格式（带 `area` label）。
2. `LevelCache::otel_metric_points()`：导出 OpenTelemetry 友好的点位结构（`name/value/attributes`）。

稳定指标名统一前缀：`accelerator_cache_*`（例如 `accelerator_cache_local_hit_total`）。

### 10.2 日志（Logging）

当前已统一操作日志字段（target=`accelerator::ops`）：

- `area`
- `op`
- `key_hash`
- `result`
- `latency_ms`
- `error_kind`

补充说明：

1. `key_hash` 来自编码后 key 的 hash，不输出原始敏感 key/value。
2. `error_kind` 由 `CacheError::kind()` 提供稳定分类：`invalid_config/backend/loader/timeout/none`。

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

### 10.4 运行时诊断接口（只读）

新增 `LevelCache::diagnostic_snapshot()`，返回以下信息：

1. 生效配置快照（`CacheConfig`）。
2. 计数器快照（`CacheMetricsSnapshot`）。
3. 运行状态：local/remote/loader 是否就绪、失效监听器是否已启动、监听 channel（若适用）。

该接口用于线上排障和配置核对，不暴露可变操作能力。

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

### 12.3 运维排障手册

已补充专门排障文档：`docs/cache-ops-runbook.md`，覆盖以下常见路径：

1. Redis 不可达（读写失败/连接异常）。
2. 失效广播异常（Pub/Sub 订阅或消费失败）。
3. 回源超时（loader timeout）。

建议将该文档纳入发布 checklist 与值班排障入口。

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

- [x] Warm up 预热机制
- [x] 失效广播（Redis Pub/Sub）
- [x] refresh ahead
- [x] 错误分级降级策略（stale_on_error）

### Milestone 3（体验）

- [x] 宏注解 API（V1 单 key：`cacheable/cache_put/cache_evict`）
- [x] 批量接口（`mget/mset/mdel` 已落地）
- [ ] 治理/观测控制面

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
3. 结合 `warmup` 在启动阶段预热热点 key。
4. 需要跨实例 L1 清理时启用 `broadcast_invalidation`。
5. 可用 `refresh_ahead + stale_on_error` 提升故障期可用性。
6. 使用 `loader_fn` 定义最小回源路径，并通过 `ReadOptions.disable_load` 控制自动回源。

生产场景推荐使用“`Repo` 实例 + `sqlx` + `Loader/MLoader` 嵌入式适配”模式，避免侵入业务查询函数。

```rust
use std::collections::HashMap;
use std::sync::Arc;

use accelerator::loader::{Loader, MLoader};
use accelerator::{CacheError, CacheResult};
use sqlx::PgPool;

#[derive(Clone, Debug)]
pub struct User {
    pub id: u64,
    pub name: String,
}

#[derive(Clone)]
pub struct UserRepo {
    pool: PgPool,
}

impl UserRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn find_by_id(&self, id: u64) -> Result<Option<User>, sqlx::Error> {
        let row = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, name FROM users WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|(id, name)| User {
            id: id as u64,
            name,
        }))
    }

    pub async fn find_by_ids(&self, ids: &[u64]) -> Result<HashMap<u64, User>, sqlx::Error> {
        let ids_i64 = ids.iter().map(|id| *id as i64).collect::<Vec<_>>();
        let rows = sqlx::query_as::<_, (i64, String)>(
            "SELECT id, name FROM users WHERE id = ANY($1::bigint[])",
        )
        .bind(&ids_i64)
        .fetch_all(&self.pool)
        .await?;

        let mut found = HashMap::with_capacity(rows.len());
        for (id, name) in rows {
            found.insert(
                id as u64,
                User {
                    id: id as u64,
                    name,
                },
            );
        }
        Ok(found)
    }
}

#[derive(Clone)]
pub struct UserRepoLoader {
    repo: Arc<UserRepo>,
}

impl UserRepoLoader {
    pub fn new(repo: Arc<UserRepo>) -> Self {
        Self { repo }
    }
}

impl Loader<u64, User> for UserRepoLoader {
    async fn load(&self, key: &u64) -> CacheResult<Option<User>> {
        self.repo
            .find_by_id(*key)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_id failed: {err}")))
    }
}

impl MLoader<u64, User> for UserRepoLoader {
    async fn mload(&self, keys: &[u64]) -> CacheResult<HashMap<u64, Option<User>>> {
        if keys.is_empty() {
            return Ok(HashMap::new());
        }

        let found = self
            .repo
            .find_by_ids(keys)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_ids failed: {err}")))?;

        let mut values = HashMap::with_capacity(keys.len());
        for key in keys {
            values.insert(*key, found.get(key).cloned());
        }
        Ok(values)
    }
}
```

这个模板的关键点：

1. `Repo` 负责纯查询语义，`Loader/MLoader` 只做缓存协议适配与错误映射。
2. 单查/批查函数可独立复用，不会因缓存引入而污染业务数据访问层。
3. `mload` 返回 `HashMap<K, Option<V>>`，明确表达“命中”和“未命中”两类结果。

下面给一段“直接可复制”的最小组装片段，把 `LevelCacheBuilder + UserRepoLoader` 串成完整可运行示例：

```rust
use std::sync::Arc;
use std::time::Duration;

use accelerator::builder::LevelCacheBuilder;
use accelerator::cache::ReadOptions;
use accelerator::config::CacheMode;
use accelerator::{local, remote};
use sqlx::postgres::PgPoolOptions;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let redis_url = std::env::var("ACCELERATOR_REDIS_URL")
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let pg_dsn = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        "postgres://accelerator:accelerator@127.0.0.1:5432/accelerator".to_string()
    });

    // 1) 初始化 Repo（sqlx）
    let pg_pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&pg_dsn)
        .await?;
    let repo = Arc::new(UserRepo::new(pg_pool));

    // 2) 初始化 Loader（把 Repo 嵌入缓存回源协议）
    let loader = UserRepoLoader::new(repo.clone());

    // 3) 初始化固定后端（moka + redis）
    let l1 = local::moka::<User>().max_capacity(100_000).build()?;
    let l2 = remote::redis::<User>()
        .url(redis_url)
        .key_prefix("prod:user")
        .build()?;

    // 4) 构建 LevelCache
    let cache = LevelCacheBuilder::<u64, User, UserRepoLoader>::new()
        .area("user_profile")
        .mode(CacheMode::Both)
        .local(l1)
        .remote(l2)
        .loader(loader)
        .local_ttl(Duration::from_secs(60))
        .remote_ttl(Duration::from_secs(300))
        .null_ttl(Duration::from_secs(30))
        .penetration_protect(true)
        .loader_timeout(Duration::from_millis(200))
        .build()?;

    // 5) 业务读取（miss 自动回源；hit 直接返回）
    let one = cache.get(&1001, &ReadOptions::default()).await?;
    println!("user#1001 => {one:?}");

    // 6) 批量读取（内部走 mget -> mload -> mset）
    let batch = cache.mget(&[1001, 1002, 1003], &ReadOptions::default()).await?;
    println!("batch => {batch:?}");

    Ok(())
}
```

> 其中 `User`、`UserRepo`、`UserRepoLoader` 直接复用上一段模板定义即可；如果你本地按 `scripts/docker-compose.yml` 启动，可直接跑通这段流程。

为了对照“手写 API”与“宏 API”的关系，下面给出同一组装方式下的服务层写法。  
前提：继续复用上面构建出的 `cache` 与 `repo`（即 `Arc<LevelCache<...>>` + `Arc<UserRepo>`）。

### 16.2.1 手写 API vs 宏 API 对照（同一 Builder）

```rust
use std::collections::HashMap;
use std::sync::Arc;

use accelerator::cache::{LevelCache, ReadOptions};
use accelerator::macros::{cacheable, cacheable_batch};
use accelerator::{CacheError, CacheResult};

#[derive(Clone)]
pub struct UserService {
    repo: Arc<UserRepo>,
    user_cache: Arc<LevelCache<u64, User, UserRepoLoader>>,
}

impl UserService {
    pub fn new(repo: Arc<UserRepo>, user_cache: Arc<LevelCache<u64, User, UserRepoLoader>>) -> Self {
        Self { repo, user_cache }
    }
}
```

#### A) 手写 API（显式调用 get/mget/set/mset）

```rust
impl UserService {
    pub async fn get_user_manual(&self, user_id: u64) -> CacheResult<Option<User>> {
        if let Some(hit) = self
            .user_cache
            .get(
                &user_id,
                &ReadOptions {
                    allow_stale: false,
                    disable_load: true,
                },
            )
            .await?
        {
            return Ok(Some(hit));
        }

        let loaded = self
            .repo
            .find_by_id(user_id)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_id failed: {err}")))?;

        self.user_cache.set(&user_id, loaded.clone()).await?;
        Ok(loaded)
    }

    pub async fn batch_get_users_manual(
        &self,
        user_ids: Vec<u64>,
    ) -> CacheResult<HashMap<u64, Option<User>>> {
        let mut cached = self
            .user_cache
            .mget(
                &user_ids,
                &ReadOptions {
                    allow_stale: false,
                    disable_load: true,
                },
            )
            .await?;

        let misses = user_ids
            .iter()
            .filter(|id| cached.get(id).cloned().flatten().is_none())
            .copied()
            .collect::<Vec<_>>();

        if misses.is_empty() {
            return Ok(cached);
        }

        let found = self
            .repo
            .find_by_ids(&misses)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_ids failed: {err}")))?;

        let mut write_back = Vec::with_capacity(misses.len());
        for id in misses {
            let value = found.get(&id).cloned();
            cached.insert(id, value.clone());
            write_back.push((id, value));
        }
        self.user_cache.mset(&write_back).await?;

        Ok(cached)
    }
}
```

#### B) 宏 API（用注解替代显式缓存编排）

```rust
impl UserService {
    #[cacheable(cache = self.user_cache, key = user_id, allow_stale = false)]
    pub async fn get_user_macro(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.repo
            .find_by_id(user_id)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_id failed: {err}")))
    }

    #[cacheable_batch(cache = self.user_cache, keys = user_ids, allow_stale = false)]
    pub async fn batch_get_users_macro(
        &self,
        user_ids: Vec<u64>,
    ) -> CacheResult<HashMap<u64, Option<User>>> {
        let found = self
            .repo
            .find_by_ids(&user_ids)
            .await
            .map_err(|err| CacheError::Loader(format!("find_by_ids failed: {err}")))?;

        let mut values = HashMap::with_capacity(user_ids.len());
        for id in user_ids {
            values.insert(id, found.get(&id).cloned());
        }
        Ok(values)
    }
}
```

一一映射关系：

1. `cache.get(... disable_load=true)` ↔ `#[cacheable(...)]` 的前置读缓存。
2. miss 后调用 `repo.find_by_id` ↔ `#[cacheable]` 包裹的方法体（仅保留业务回源）。
3. `cache.set` 回写 ↔ `#[cacheable]` 自动回写。
4. `cache.mget(... disable_load=true)` + misses 计算 ↔ `#[cacheable_batch]` 自动处理 misses。
5. `cache.mset` 回写批量结果 ↔ `#[cacheable_batch]` 自动批量回写。

### 16.3 集成测试策略（Redis / Postgres）

当前版本已提供两类真实环境集成测试：

1. Redis 集成测试：`tests/redis_integration.rs`
2. Redis + Postgres 联合集成测试：`tests/stack_integration.rs`（Postgres 回源通过 `sqlx`）

测试约束：

1. 每个用例运行前先探测 Redis 可达性（`PING`）。
2. 联合集成测试会同时探测 Postgres 可达性；任一不可达时自动跳过对应用例。
3. 默认地址：
   - Redis：`redis://127.0.0.1:6379`
   - Postgres：`postgres://accelerator:accelerator@127.0.0.1:5432/accelerator`
4. 可通过环境变量覆盖：
   - `ACCELERATOR_TEST_REDIS_URL`
   - `ACCELERATOR_TEST_POSTGRES_DSN`

推荐执行方式：

```bash
cargo test
```

若本地环境按 `scripts/docker-compose.yml` 启动，将同时执行单元测试与集成测试。  
本地环境启动与链路查看说明见：`docs/local-stack-integration.md`。

### 16.4 当前版本边界说明

- 过程宏 V2 已落地，当前支持单 key + 批量注解：`cacheable` / `cache_put` / `cache_evict` / `cacheable_batch` / `cache_evict_batch`。
- 宏参数校验与签名报错已增强，覆盖 `async method`、重复参数、未知参数、`on_cache_error` 取值等常见误用。
- 治理与观测控制面基础能力已落地：指标导出适配、标准日志字段、运行时诊断快照、运维手册。
- `listener/tag` 风格注解尚未实现，仍作为后续增强项。

---

## 17. 过程宏方案（HyPerf 风格，V2 已实现）

本节给出“完整可落地”的宏方案草案，用于提升业务代码体验。  
设计目标是参考 HyPerf 的意图分层（`Cacheable/Put/Evict`），但采用 Rust 编译期展开与类型检查语义。

### 17.1 设计目标

1. 保持现有公共 API 不变：`get/set/del/mget/mset/mdel` 仍是唯一运行时标准接口。
2. 宏仅做“语法糖包装”，不复制缓存引擎逻辑。
3. 使用 Rust 表达式生成 key/value，不引入 SpEL/字符串 DSL。
4. 单 key 与批量路径都通过过程宏统一，业务只保留查询/写入核心逻辑。
5. 与现有能力对齐：singleflight、ttl-jitter、refresh-ahead、stale-on-error、广播失效均继续由 `LevelCache` 配置生效。

### 17.2 宏列表与分阶段范围

V1（首批）：

1. `#[cacheable(...)]`：读缓存，miss 执行函数并回填。
2. `#[cache_put(...)]`：执行业务后写缓存。
3. `#[cache_evict(...)]`：执行业务后删缓存（可选 before）。

V2（增强）：

1. `#[cacheable_batch(...)]`：批量读取包装（内部调用 `mget/mset`）。（已实现）
2. `#[cache_evict_batch(...)]`：批量失效包装（内部调用 `mdel`）。（已实现）
3. `listener/tag` 风格失效主题（映射到广播通道）。（待实现）

### 17.3 Key 构建契约（必须统一）

宏层与内核层采用“两段式 key”：

1. **宏层生成业务 key（`K`）**：由 `key = ...` Rust 表达式产生。
2. **内核层生成最终缓存 key（`CacheKey`）**：`LevelCache` 统一执行 `area + ":" + key_converter(K)`。

约束：

1. 宏层不得自行拼接最终 `area` 前缀，避免与 `LevelCache::encoded_key` 语义冲突。
2. 业务若需要额外维度（如 tenant/version），在 `key` 表达式中编码到 `K` 或 `key_converter` 中。
3. 文档与示例都应明确“宏 key 不是最终 Redis key”。

### 17.4 V1 宏参数草案

#### 17.4.1 `#[cacheable(...)]`

```rust
#[cacheable(
    cache = self.user_cache,
    key = user_id,
    allow_stale = false,
    cache_none = true,
    on_cache_error = "ignore"
)]
```

参数说明：

1. `cache`：缓存实例表达式（需支持 `get/set`）。
2. `key`：业务 key 表达式（类型匹配 `K`）。
3. `allow_stale`：映射 `ReadOptions.allow_stale`（默认 `false`）。
4. `cache_none`：函数返回 `None` 时是否写空值缓存（默认 `true`）。
5. `on_cache_error`：`"ignore"` 或 `"propagate"`（默认 `"ignore"`）。

#### 17.4.2 `#[cache_put(...)]`

```rust
#[cache_put(
    cache = self.user_cache,
    key = user.id,
    value = Some(user.clone()),
    on_cache_error = "ignore"
)]
```

参数说明：

1. `cache`：缓存实例表达式（需支持 `set`）。
2. `key`：业务 key 表达式。
3. `value`：写缓存值表达式（类型匹配 `Option<V>`）。
4. `on_cache_error`：`"ignore"` 或 `"propagate"`（默认 `"ignore"`）。

#### 17.4.3 `#[cache_evict(...)]`

```rust
#[cache_evict(
    cache = self.user_cache,
    key = user_id,
    before = false,
    on_cache_error = "ignore"
)]
```

参数说明：

1. `cache`：缓存实例表达式（需支持 `del`）。
2. `key`：业务 key 表达式。
3. `before`：是否在执行业务前删除缓存（默认 `false`，推荐后删）。
4. `on_cache_error`：`"ignore"` 或 `"propagate"`（默认 `"ignore"`）。

#### 17.4.4 `#[cacheable_batch(...)]`

```rust
#[cacheable_batch(
    cache = self.user_cache,
    keys = user_ids,
    allow_stale = false,
    on_cache_error = "ignore"
)]
```

参数说明：

1. `cache`：缓存实例表达式（需支持 `mget/mset`）。
2. `keys`：批量 key 表达式（建议直接使用方法参数标识符）。
3. `allow_stale`：映射到 `ReadOptions.allow_stale`（默认 `false`）。
4. `on_cache_error`：`"ignore"` 或 `"propagate"`（默认 `"ignore"`）。

#### 17.4.5 `#[cache_evict_batch(...)]`

```rust
#[cache_evict_batch(
    cache = self.user_cache,
    keys = user_ids,
    before = false,
    on_cache_error = "ignore"
)]
```

参数说明：

1. `cache`：缓存实例表达式（需支持 `mdel`）。
2. `keys`：批量 key 表达式。
3. `before`：是否在执行业务前先批量失效（默认 `false`）。
4. `on_cache_error`：`"ignore"` 或 `"propagate"`（默认 `"ignore"`）。

### 17.5 宏展开语义（V1 + V2）

#### 17.5.1 `cacheable`

1. 先执行 `cache.get(key, ReadOptions { disable_load: true, allow_stale })`。
2. 命中直接返回。
3. miss 时执行业务函数体（等价“手写回源”）。
4. 业务返回成功后：
   - `Some(v)` -> `cache.set(key, Some(v.clone()))`
   - `None` 且 `cache_none=true` -> `cache.set(key, None)`
5. 返回业务函数原始结果。

#### 17.5.2 `cache_put`

1. 执行业务函数体。
2. 成功后执行 `cache.set(key, value)`。
3. 返回业务函数原始结果。

#### 17.5.3 `cache_evict`

1. `before=true`：先 `cache.del(key)`，再执行业务函数。
2. `before=false`：先执行业务函数，成功后 `cache.del(key)`。
3. 返回业务函数原始结果。

#### 17.5.4 `cacheable_batch`

1. 先对 `keys` 去重（按 `HashMap` 语义去重）。
2. 执行 `cache.mget(keys, ReadOptions { disable_load: true, allow_stale })`。
3. 把值为 `None` 的 key 视作 misses；命中值直接保留。
4. 仅将 misses 传递给业务函数体（当 `keys` 是方法参数标识符且类型为 `Vec<K>` / `&[K]` / `&Vec<K>` 时自动重绑）。
5. 业务函数成功后，将 misses 的结果回写 `cache.mset(...)`，并返回完整 `HashMap<K, Option<V>>`。

#### 17.5.5 `cache_evict_batch`

1. 先对 `keys` 去重。
2. `before=true`：先 `cache.mdel(&keys)`，再执行业务函数。
3. `before=false`：先执行业务函数，成功后 `cache.mdel(&keys)`。
4. 返回业务函数原始结果。

### 17.6 类型与签名约束（V2）

1. 仅支持 `async fn`。
2. 仅支持方法场景（含 `&self` / `&mut self`），free fn 放到后续版本。
3. 仅接受“result-like”返回类型（例如 `Result<T, E>`、`CacheResult<T>`、`MacroTestResult<T>`）。
4. `cacheable` 要求 Ok 类型为 `Option<V>`；`cacheable_batch` 要求 Ok 类型为 `HashMap<K, Option<V>>`。
5. `cache_put/cache_evict/cache_evict_batch` 对 Ok 类型不做额外限制。
6. `cacheable_batch` 在“misses 自动重绑”场景下，`keys` 参数类型需为 `Vec<K>` / `&[K]` / `&Vec<K>`。
7. 若签名不匹配，宏在编译期报错，优先给出“可执行修改建议”（例如改为 `async fn ...(&self, ...)`）。
8. 当 `on_cache_error="propagate"` 时，返回错误类型需满足 `From<CacheError>`。

### 17.7 批量宏（V2）当前语义

1. `cacheable_batch` 与 `cache_evict_batch` 都会先对输入 key 去重（按 `HashMap` 语义）。
2. `cacheable_batch` 默认返回覆盖请求 key 的 `HashMap<K, Option<V>>`，并在 miss 后批量 `mset` 回写。
3. misses 判断基于 `mget(disable_load=true)` 返回中的 `None` 值。
4. 业务函数返回缺失 key 时，宏会按 `None` 合并并回写，保证返回结构完整。

### 17.8 错误语义与可观测

1. 默认 `on_cache_error="ignore"`：缓存异常不阻断业务主链路（fail-open）。
2. 可选 `on_cache_error="propagate"`：缓存异常直接返回错误。
3. 宏展开路径统一输出 `tracing` 字段：`cache_macro`, `op`, `result`, `error`。
4. `result` 当前固定为 `ignored`/`propagated`，便于在日志平台快速聚合缓存异常处理策略。
5. 与 `metrics_snapshot()` 配合，保证可定位“业务失败”与“缓存失败”的边界。

### 17.9 crate 组织建议

1. 对外统一使用 `accelerator::macros` 命名空间，不要求业务方直接依赖额外 crate。
2. 实现上在 workspace 内保留内部 `macros/` proc-macro 子包，并由主 crate 进行 re-export：

```rust
pub mod macros {
    pub use macros_impl::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};
}
```

3. 业务侧期望导入方式：

```rust
use accelerator::macros::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};
```

4. 生成代码仅依赖 `accelerator` 公共 API，不访问内部私有实现。

### 17.10 最小示例（V1 目标形态）

```rust
use accelerator::macros::{cache_evict, cacheable};
use accelerator::CacheResult;

impl UserService {
    #[cacheable(cache = self.user_cache, key = user_id)]
    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.repo.find_user(user_id).await
    }

    #[cache_evict(cache = self.user_cache, key = user_id)]
    async fn delete_user(&self, user_id: u64) -> CacheResult<()> {
        self.repo.delete_user(user_id).await
    }
}
```

该示例的目标是把“缓存模板代码”从业务方法中剥离，同时保持行为可预测、可测试、可观测。

### 17.11 宏展开前后对照（V1.1）

业务写法（宏前）：

```rust
async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
    let opts = ReadOptions { allow_stale: false, disable_load: true };
    if let Some(hit) = self.cache.get(&user_id, &opts).await? {
        return Ok(Some(hit));
    }

    let loaded = self.repo.find_by_id(user_id).await?;
    if let Some(v) = loaded.as_ref() {
        self.cache.set(&user_id, Some(v.clone())).await?;
    }
    Ok(loaded)
}
```

宏写法（宏后）：

```rust
#[cacheable(cache = self.cache, key = user_id, on_cache_error = "ignore")]
async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
    self.repo.find_by_id(user_id).await
}
```

推荐直接参考可运行示例：`examples/macro_best_practice.rs`。

### 17.12 批量宏示例（V2）

批量宏完整示例见：`examples/macro_batch_best_practice.rs`。

该示例覆盖：

1. `#[cacheable_batch]`：`mget` + misses 回源 + `mset` 回写。
2. `#[cache_evict_batch]`：批量删除后统一失效。
3. 重复 key 去重后的返回语义（`HashMap<K, Option<V>>`）。

---

## 18. 后续计划（详细 Roadmap）

本节给出 V1 之后的详细实施计划，目标是把“可用”推进到“可规模化落地”。

### 18.1 迭代 A：宏 V1.1 稳定化（短期）

目标：提升宏的可用性与可诊断性，不改变核心运行时语义。

交付项：

1. **签名检查增强**：对 `async fn`、返回类型、参数类型给出更清晰的编译期报错。
2. **参数校验增强**：完善 `cache/key/value/on_cache_error/before` 的错误提示与示例建议。
3. **测试矩阵扩展**：
   - `on_cache_error=ignore/propagate` 双路径测试
   - `cache_none=true/false` 行为测试
   - `before=true/false` + 业务失败/成功组合测试
4. **文档与示例补齐**：
   - 增加 `examples/macro_best_practice.rs`
   - 补充“宏展开前后对照示例”
5. **可观测增强**：宏展开路径补充统一 `tracing` 字段约定。

验收标准：

1. `cargo test` 全绿，宏测试覆盖率显著提升（重点分支全部覆盖）。
2. 常见误用（参数拼错/签名错误）能够在编译期直接定位。
3. 宏示例可直接运行，并与手写缓存代码语义一致。

当前状态（2026-03-07）：

1. [x] 签名/参数校验增强已落地（`macros/src/lib.rs`）。
2. [x] 宏测试矩阵已补齐并拆分到独立模块（`tests/macro_v1/*.rs`）。
3. [x] 过程宏最佳实践示例已补充（`examples/macro_best_practice.rs`）。
4. [x] 宏路径日志字段已统一为 `cache_macro/op/result/error`。

### 18.2 迭代 B：批量宏 V2（中期）

目标：补齐批量场景宏能力，同时保持现有 `HashMap<K, Option<V>>` 语义不变。

交付项：

1. 新增 `#[cacheable_batch(...)]`：
   - 先 `mget(disable_load=true)`，再对 misses 执行业务批量回源
   - 回写后返回覆盖全部请求 key 的 `HashMap<K, Option<V>>`
2. 新增 `#[cache_evict_batch(...)]`：
   - 由 `keys=...` 产出 `Vec<K>`，统一调用 `mdel(&keys)`
3. 明确重复 key 语义：
   - 默认按 `HashMap` 语义去重
   - 文档中明确“重复 key 不保证重复返回”
4. 批量路径测试扩展：
   - 大批量 misses/hits 混合
   - 重复 key
   - 空输入
   - 部分回源失败

验收标准：

1. 批量宏在语义上与手写 `mget/mset/mdel` 主流程一致。
2. 性能开销可控（与手写实现相比不出现数量级退化）。
3. 文档中明确边界条件与重复 key 行为。

当前状态（2026-03-07）：

1. [x] `cacheable_batch` 已实现（`mget` -> miss 合并 -> `mset`）。
2. [x] `cache_evict_batch` 已实现（支持 `before=true/false`）。
3. [x] 重复 key 已按 `HashMap` 语义去重。
4. [x] 批量测试矩阵已补齐（`tests/macro_v2/*.rs`）。
5. [x] 批量最佳实践示例已补充（`examples/macro_batch_best_practice.rs`）。

### 18.3 迭代 C：治理与观测控制面（中期）

目标：让组件从“库可用”走向“线上可治理”。

交付项：

1. 指标出口能力（Prometheus/OpenTelemetry 适配）：
   - 将 `metrics_snapshot` 结构化映射为稳定指标命名
2. 关键事件日志标准化：
   - 统一字段：`area/op/key_hash/result/latency_ms/error_kind`
3. 运行时诊断接口（只读）：
   - 当前配置快照
   - 关键计数器快照
4. 运维手册：
   - Redis 不可达、广播异常、回源超时的排障路径

验收标准：

1. 故障注入场景下可通过指标+日志快速定位根因。
2. 线上排障文档可覆盖 80% 常见问题路径。

当前状态（2026-03-07）：

1. [x] 指标出口适配已提供（`prometheus_metrics` / `otel_metric_points`）。
2. [x] 关键操作日志字段已统一（`area/op/key_hash/result/latency_ms/error_kind`）。
3. [x] 运行时诊断接口已提供（`diagnostic_snapshot`）。
4. [x] 运维排障手册已补充（`docs/cache-ops-runbook.md`）。

### 18.4 迭代 D：性能与工程化收敛（中长期）

目标：将宏能力与核心缓存路径在性能与工程规范上长期稳定化。

交付项：

1. 基准测试（`criterion`）：
   - 手写路径 vs 宏路径（单 key / 批量）
   - 本地命中 / 远端命中 / miss 回源 三种场景
2. 回归门禁：
   - 为关键 benchmark 设回归阈值
3. API 稳定策略：
   - 宏参数向后兼容规则
   - 破坏性修改的迁移说明模板
4. 示例工程：
   - 提供一套“推荐项目结构 + 宏使用模板”

验收标准：

1. 宏路径性能与手写路径差距在可接受范围内（有量化结论）。
2. 每次发布具备可追踪的兼容性说明与迁移指导。

当前状态（2026-03-07）：

1. [x] `criterion` 基准测试已提供（`benches/cache_path_bench.rs`）。
   - 默认覆盖本地命中 + miss 回源；远端命中在 Redis 可用时可选执行（`ACCELERATOR_BENCH_REDIS_URL`）。
2. [x] 回归门禁工具已提供（`src/bin/check_bench_regression.rs`，支持阈值配置）。
3. [x] Benchmark 基线快照已落地（`docs/benchmarks/cache_path_bench.json`）。
4. [x] API 稳定策略 + 迁移模板 + 推荐项目结构已补充（`docs/performance-engineering-playbook.md`）。

### 18.5 执行顺序建议

1. 已完成 **迭代 A（V1.1 稳定化）**、**迭代 B（批量宏 V2）**、**迭代 C（治理与观测控制面）**、**迭代 D（性能与工程化收敛）**。
2. 后续以“持续治理”方式维护：每次版本迭代都执行 benchmark 回归门禁。
3. 发布流程建议固定执行：`cargo test` + `cargo bench` + `cargo run --bin check_bench_regression -- --threshold 0.15` + runbook 自检。

### 18.6 风险与回退策略

1. **宏误判签名风险**：通过 compile-fail 测试与错误信息模板化降低风险。
2. **批量语义歧义风险**：固定默认 `HashMap` 语义并在文档中明确重复 key 行为。
3. **性能回退风险**：引入 benchmark 基线与 CI 阈值告警。
4. **线上行为不可见风险**：优先建设可观测能力，再扩大宏覆盖面。
