# Multi-Level Cache Capability Model (Rust, JetCache-Inspired)

> Terminology baseline: see [docs/terminology.md](./terminology.md).

## 1. Document Purpose

This document defines the capability model for a pluggable, production-oriented
multi-level cache runtime in Rust. It is the baseline for API design,
implementation boundaries, tests, and operations.

Goals:

1. Keep runtime behavior predictable and observable.
2. Provide a stable abstraction over local (L1) and remote (L2) cache layers.
3. Support incremental capability rollout without redesigning core APIs.

---

## 2. Design Principles

1. **Abstraction first**: define contracts before implementation details.
2. **Default backend first**: ship stable defaults (`moka` + `redis`) and expose extension points.
3. **Rust-native async model**: avoid `async_trait`; use trait methods returning `impl Future`.
4. **Correctness before peak performance**: optimize only after semantics are clear.
5. **Observability by default**: metrics, logs, and trace fields are first-class.
6. **Static dispatch by default**: prefer generics over runtime trait objects.

### 2.1 MVP constraints in current implementation

1. Primary runtime type: `LevelCache<K, V, LD, LB, RB>`
2. Builder entry: `LevelCacheBuilder<K, V, LD, LB, RB>`
3. Default backends: `moka` for local, `redis` for remote
4. `mget` result covers all requested keys: `HashMap<K, Option<V>>`
5. `singleflight-async` is used for single-key miss-load dedup
6. `ttl_jitter_ratio` is implemented to reduce avalanche risk
7. Built-in capabilities include `warmup`, `refresh_ahead`, `stale_on_error`, pub/sub invalidation

---

## 3. System Boundaries

### 3.1 In scope

- Unified orchestration over local and remote cache layers
- Read/write/delete APIs (`get/mget/set/mset/del/mdel`)
- Miss loading and batch loading behavior
- Consistency-related strategies (negative cache, invalidation broadcast)
- Runtime diagnostics and observability output

### 3.2 Out of scope

- Database transaction guarantees
- Cross-region conflict resolution
- Strong consistency locking systems
- Framework-specific glue code (Axum, Actix, Tonic, etc.)

---

## 4. Core Concept Model

### 4.1 Layer model

- **L1 (Local)**: process-local, low latency, bounded memory
- **L2 (Remote)**: shared across instances, larger capacity, higher latency
- **Loader**: source-of-truth fetch on cache miss
- **Invalidation channel**: cross-instance L1 invalidation signal

### 4.2 Data entities

- **Cache key**: namespaced key (`area + business key`)
- **Stored entry**: payload + absolute expire time
- **Stored value**: `Value(V)` or `Null` (negative cache marker)
- **Read options**: stale allowance and load skip switch

### 4.3 Runtime states

1. `Absent`
2. `HitFresh`
3. `HitStale`
4. `Loading`
5. `Invalidated`

---

## 5. Interface Capability Model

### 5.1 Layered responsibilities

1. **Storage layer**
   - `LocalBackend<V>`
   - `RemoteBackend<V>`
   - `InvalidationSubscriber`
2. **Orchestration layer**
   - `LevelCache<K, V, LD, LB, RB>`
3. **Loader layer**
   - `Loader<K, V>`
   - `MLoader<K, V>`

### 5.2 Core types (contract)

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
    pub area: String,
    pub mode: CacheMode,
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
}
```

Contract notes:

1. `StoredEntry` must carry expiration metadata.
2. `StoredValue::Null` and miss are different semantics.
3. `disable_load=true` means strict cache-only behavior.

### 5.3 Ownership and copy semantics

Current default behavior is owned clone semantics for safety and predictability.
Potential shared-reference modes are optional future extensions.

### 5.4 Backend abstractions

```rust
pub trait LocalBackend<V>: Clone + Send + Sync + 'static {
    fn get(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;
    fn peek(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;
    fn mget(&self, keys: &[String])
        -> impl Future<Output = CacheResult<HashMap<String, Option<StoredEntry<V>>>>> + Send;
    fn set(&self, key: &str, entry: StoredEntry<V>)
        -> impl Future<Output = CacheResult<()>> + Send;
    fn mset(&self, entries: HashMap<String, StoredEntry<V>>)
        -> impl Future<Output = CacheResult<()>> + Send;
    fn del(&self, key: &str) -> impl Future<Output = CacheResult<()>> + Send;
    fn mdel(&self, keys: &[String]) -> impl Future<Output = CacheResult<()>> + Send;
}

pub trait InvalidationSubscriber: Send {
    fn next_message(&mut self) -> impl Future<Output = Option<CacheResult<String>>> + Send;
}

pub trait RemoteBackend<V>: Clone + Send + Sync + 'static {
    type Subscriber: InvalidationSubscriber + Send + 'static;

    fn get(&self, key: &str) -> impl Future<Output = CacheResult<Option<StoredEntry<V>>>> + Send;
    fn mget(&self, keys: &[String])
        -> impl Future<Output = CacheResult<HashMap<String, Option<StoredEntry<V>>>>> + Send;
    fn set(&self, key: &str, entry: StoredEntry<V>)
        -> impl Future<Output = CacheResult<()>> + Send;
    fn mset(&self, entries: HashMap<String, StoredEntry<V>>)
        -> impl Future<Output = CacheResult<()>> + Send;
    fn del(&self, key: &str) -> impl Future<Output = CacheResult<()>> + Send;
    fn mdel(&self, keys: &[String]) -> impl Future<Output = CacheResult<()>> + Send;
    fn publish(&self, channel: &str, payload: &str)
        -> impl Future<Output = CacheResult<()>> + Send;
    fn subscribe(&self, channel: &str)
        -> impl Future<Output = CacheResult<Self::Subscriber>> + Send;
}
```

Semantics:

1. `mget` must return entries for all requested keys (`Some` or `None`).
2. `del`/`mdel` should be idempotent.
3. Batch APIs should use real backend batch primitives when available.

### 5.5 Loader and orchestration APIs

```rust
pub trait Loader<K, V>: Send + Sync {
    fn load(&self, key: &K) -> impl Future<Output = CacheResult<Option<V>>> + Send;
}

pub trait MLoader<K, V>: Loader<K, V> {
    fn mload(&self, keys: &[K]) -> impl Future<Output = CacheResult<HashMap<K, Option<V>>>> + Send;
}
```

```rust
pub struct LevelCache<K, V, LD = NoopLoader, LB = MokaBackend<V>, RB = RedisBackend<V>> { /* ... */ }

impl<K, V, LD, LB, RB> LevelCache<K, V, LD, LB, RB> {
    pub async fn get(&self, key: &K, opts: &ReadOptions) -> CacheResult<Option<V>>;
    pub async fn mget(&self, keys: &[K], opts: &ReadOptions) -> CacheResult<HashMap<K, Option<V>>>;
    pub async fn set(&self, key: &K, value: Option<V>) -> CacheResult<()>;
    pub async fn mset(&self, values: HashMap<K, Option<V>>) -> CacheResult<()>;
    pub async fn del(&self, key: &K) -> CacheResult<()>;
    pub async fn mdel(&self, keys: &[K]) -> CacheResult<()>;
    pub async fn warmup(&self, keys: &[K]) -> CacheResult<usize>;
}
```

### 5.5.1 singleflight convention

1. Singleflight dedup applies to single-key miss-load paths.
2. `mget` miss handling uses `MLoader::mload` directly.
3. Singleflight dedup key must be the normalized encoded key.

### 5.6 Extension interfaces

Current extension points:

1. Backend adapters
2. Key converter
3. Loader adapter
4. Observability exporters

### 5.7 Error model

Suggested categories:

1. Config errors
2. Backend errors
3. Loader errors
4. Timeout errors
5. Serialization errors

### 5.8 Async conventions

1. Traits use `fn -> impl Future` style.
2. Implementation methods can still use direct `async fn`.
3. No `async_trait` dependency required.

### 5.9 Interface semantic summary

- Reads: local first, then remote, then loader (if enabled)
- Writes: follow mode (`Local`, `Remote`, `Both`) with batch-first behavior
- Deletes: remove active layers and optionally publish invalidation

---

## 6. Capability Matrix

### L0: required foundation

- Unified read/write/delete API
- Local + remote backend orchestration
- Loader and batch loader integration

### L1: high-concurrency stability

- Singleflight for single-key miss load
- Negative cache
- TTL jitter

### L2: consistency enhancement

- Pub/Sub invalidation broadcast
- Refresh-ahead
- Stale fallback on source errors

### L3: usability and engineering

- Macros for common patterns
- Benchmark baseline and regression gate
- Runbooks and integration guides

---

## 7. Flow Models

### 7.1 Read flow (`Both` mode)

1. Try local read
2. On local miss, try remote read
3. On remote hit, backfill local
4. On miss, load from source if allowed
5. Write loaded result back by mode

### 7.2 Write flow (recommended: DB first, then cache invalidation/write)

1. Commit source-of-truth change
2. Invalidate or rewrite cache according to chosen strategy
3. Publish invalidation when cross-instance L1 consistency is required

### 7.3 Delete flow

1. Delete active cache layers
2. Publish invalidation (if enabled)

### 7.3.1 Pub/Sub timeline (A deletes, B clears L1)

1. A executes `del/mdel`
2. A publishes invalidation message for encoded keys
3. B subscriber receives message
4. B clears corresponding local keys

---

## 8. Consistency Strategy Model

### 8.1 Consistency levels

1. Local-only eventual consistency
2. Cross-instance eventual consistency with broadcast invalidation
3. Source-of-truth-first write discipline

### 8.2 Concurrent update model

1. Accept eventual consistency by default
2. Prefer idempotent write/delete operations
3. Define application-level conflict policy in service layer

---

## 9. Configuration Model

Suggested key knobs:

1. `mode`
2. `local_ttl`, `remote_ttl`, `null_ttl`
3. `ttl_jitter_ratio`
4. `cache_null_value`
5. `penetration_protect`
6. `loader_timeout`
7. `refresh_ahead`, `refresh_ahead_window`
8. `stale_on_error`
9. `broadcast_invalidation`

### 9.1 Ownership defaults

Use owned-clone semantics by default for predictable behavior and simpler safety model.

### 9.2 Eviction policy defaults

For local default backend (`moka`), rely on native eviction behavior and keep runtime API simple.

---

## 10. Observability Model

### 10.1 Metrics

Core counters include:

- local hit/miss
- remote hit/miss
- load total/success/timeout/error
- refresh attempts/success/failure
- invalidation publish/receive and failures

### 10.2 Logging

Standard operation log fields should include:

- `area`
- `op`
- `result`
- `latency_ms`
- `error_kind`

### 10.3 Tracing

Add spans for core operations (`cache.get`, `cache.mget`, `cache.set`, `cache.load`, etc.).

### 10.4 Runtime diagnostics

Expose read-only snapshots:

1. `diagnostic_snapshot()`
2. `metrics_snapshot()`
3. `otel_metric_points()`

---

## 11. Extension Model

Pluggable areas:

1. Local backend implementation
2. Remote backend and subscriber implementation
3. Loader implementation and adapters
4. Key encoding strategy
5. Telemetry integration

---

## 12. Reliability and Testing Model

### 12.1 Test layers

1. Unit tests for runtime behavior and helper semantics
2. Integration tests with real Redis
3. Stack tests with Redis + Postgres + loader path
4. Benchmark regression checks

### 12.2 Fault injection scenarios

1. Redis unavailable
2. Invalid pub/sub payload
3. Loader timeout
4. Loader backend error
5. Partial batch load response

### 12.3 Ops runbook

Maintain production troubleshooting docs for:

1. Redis failures
2. Invalidation broadcast failures
3. Loader timeout and downstream slowness

---

## 13. Security and Governance

1. Do not expose mutable runtime admin controls in production by default.
2. Keep diagnostics read-only.
3. Avoid logging sensitive raw key/value payloads.
4. Keep migration and compatibility notes for public behavior changes.

---

## 14. Milestones

### Milestone 1 (MVP)

- Core read/write/delete orchestration
- Loader integration
- Basic observability

### Milestone 2 (enhancement)

- Invalidation broadcast
- Refresh-ahead and stale fallback
- Batch macro support

### Milestone 3 (experience and governance)

- Benchmark and regression gate
- Expanded runbooks and diagnostics
- Better extension and examples

---

## 15. Glossary

- **L1**: local process cache
- **L2**: shared remote cache
- **Negative cache**: storing explicit `None` result
- **Singleflight**: in-flight duplicate suppression for same key
- **Warmup**: preload keys through normal cache path
- **Refresh-ahead**: refresh near-expiration values before hard expiry

---

## 16. Public Crate Usage Model (Default Backend + Replaceable)

### 16.1 Public API shape

- Runtime type: `LevelCache<K, V, LD, LB, RB>`
- Builder type: `LevelCacheBuilder<K, V, LD, LB, RB>`
- Public modules: `backend`, `builder`, `cache`, `config`, `loader`, `local`, `remote`, `observability`

### 16.2 Builder best practice

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
    .null_ttl(Duration::from_secs(30))
    .ttl_jitter_ratio(0.1)
    .cache_null_value(true)
    .penetration_protect(true)
    .broadcast_invalidation(true)
    .build()?;
```

### 16.2.1 Handwritten API vs macro API

Both styles are supported:

1. Explicit runtime calls (`get/mget/set/mset/del/mdel`)
2. Macro-driven service methods (`cacheable/cacheable_batch/cache_put/cache_evict/cache_evict_batch`)

### 16.3 Integration test strategy (Redis / Postgres)

1. Redis tests should skip when Redis is unavailable
2. Stack tests validate real loader path with Postgres
3. Keep deterministic assertions for cache hit/miss and write-back behavior

### 16.4 Current version boundaries

1. Runtime focuses on fixed local/remote orchestration semantics
2. Macros cover common service-layer patterns, not every custom flow
3. Governance and ops controls are read-only by design

---

## 17. Procedural Macro Plan (HyPerf-style, V2 implemented)

### 17.1 Design goals

1. Remove repeated cache orchestration boilerplate
2. Preserve explicit behavior contracts
3. Keep compile-time diagnostics actionable

### 17.2 Macro list and staged scope

1. `cacheable`
2. `cache_put`
3. `cache_evict`
4. `cacheable_batch`
5. `cache_evict_batch`

### 17.3 Key construction contract

Key expression semantics in macros must match runtime key conversion semantics.

### 17.4 Macro parameter conventions (summary)

1. `cache`: cache instance expression
2. `key` / `keys`: key expression
3. `value`: write value expression (for `cache_put`)
4. `before`: pre-evict flag for evict macros
5. `on_cache_error`: `ignore` or `propagate`

### 17.5 Macro expansion semantics (summary)

- `cacheable`: cache-only read first, miss runs business logic, then cache write-back
- `cache_put`: run business logic, then write cache
- `cache_evict`: evict before or after business logic by `before` flag
- `cacheable_batch`: `mget` first, compute misses, call business function with misses, `mset` write-back
- `cache_evict_batch`: batch eviction before or after business logic by `before` flag

### 17.6 Type and signature constraints

1. Async methods only
2. Method context only (`&self` / `&mut self`)
3. Result-like return type required
4. `cacheable` expects `Result<Option<V>, E>`-style ok type
5. `cacheable_batch` expects `Result<HashMap<K, Option<V>>, E>`-style ok type

### 17.7 Current batch macro semantics

1. Input keys are de-duplicated by map semantics
2. Misses are identified from cache read results with `None` values
3. Returned map covers all requested keys

### 17.8 Error semantics and observability

1. Default `on_cache_error="ignore"` is fail-open
2. `on_cache_error="propagate"` forwards cache error
3. Macro-generated tracing fields should be stable and query-friendly

### 17.9 Crate organization recommendation

1. Expose macros through `accelerator::macros`
2. Keep proc-macro crate internal to workspace and re-export from main crate

### 17.10 Minimal macro example

```rust
#[cacheable(cache = self.cache, key = user_id, on_cache_error = "ignore")]
async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
    self.repo.get_user(user_id).await
}
```

### 17.11 Before/after comparison

1. Before: explicit cache orchestration in every method
2. After: annotation-driven orchestration with equivalent semantics

### 17.12 Batch macro example

See: `examples/macro_batch_best_practice.rs`

---

## 18. Roadmap

### 18.1 Iteration A: macro v1.1 stabilization

Focus:

1. Better compile-time diagnostics
2. Better parameter validation
3. Expanded macro test matrix

### 18.2 Iteration B: batch macro v2

Focus:

1. Stable miss-merge-write-back semantics
2. Better duplicate-key and partial-hit behavior coverage
3. Reliable batch eviction semantics

### 18.3 Iteration C: governance and observability control surface

Focus:

1. OTel metric export quality
2. Standardized operation logs
3. Read-only runtime diagnostics
4. Production troubleshooting docs

### 18.4 Iteration D: performance and engineering convergence

Focus:

1. Criterion benchmark coverage (manual vs macro)
2. Regression threshold gate
3. Stability and migration policy

### 18.5 Suggested execution order

1. Keep benchmark and regression checks in every release cycle
2. Validate runtime behavior first, then optimize
3. Keep docs, runbooks, and examples in sync with code behavior

### 18.6 Risks and rollback

1. Macro signature mismatch risk -> mitigate with compile-fail tests
2. Batch semantic ambiguity -> lock and document dedup semantics
3. Performance regression risk -> baseline + gate + repeated runs
4. Operational blind spots -> metrics + logs + diagnostics by default
