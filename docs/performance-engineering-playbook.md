# Performance and Engineering Playbook

> Terminology baseline: see [docs/terminology.md](./terminology.md).

This playbook defines benchmark commands, full load-test workflow, regression gating,
compatibility policy, and recommended project structure.

Related files:

1. Benchmark entry: `benches/cache_path_bench.rs`
2. One-click script: `scripts/bench.sh`
3. Baseline exporter: `src/bin/export_bench_baseline.rs`
4. Regression checker: `src/bin/check_bench_regression.rs`

## 1. Benchmarking (criterion)

### 1.1 Coverage matrix

Current benchmark file: `benches/cache_path_bench.rs`

Covered scenarios:

1. Single-key hit: `single_key/hit/manual` vs `single_key/hit/macro`
2. Single-key miss with miss load (source-of-truth load): `single_key/miss/manual` vs `single_key/miss/macro`
3. Batch hit: `batch/hit/manual/32` vs `batch/hit/macro/32`
4. Batch miss with miss load (source-of-truth load): `batch/miss/manual/32` vs `batch/miss/macro/32`

Notes:

1. Local (`moka`) benchmarks always run first.
2. Remote (`redis`) benchmarks run when `ACCELERATOR_BENCH_REDIS_URL` is reachable.
3. If Redis endpoint is invalid/unreachable, remote benchmarks may fail; use local-only mode to bypass.

### 1.2 Pre-run checklist

1. Ensure correctness first: `cargo test`
2. Keep host environment stable (power plugged in, low background load)
3. Verify Redis availability before remote benchmarks (`redis-cli ping`)

### 1.3 Raw command list (without script)

Single formal run:

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
```

Local-only run (skip Redis path):

```bash
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 \
cargo bench --bench cache_path_bench -- --sample-size=60
```

Run with Redis path enabled:

```bash
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:6379 \
cargo bench --bench cache_path_bench -- --sample-size=60
```

Repeat 3 times and save logs:

```bash
mkdir -p target/bench-logs
for i in 1 2 3; do
  ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 \
  cargo bench --bench cache_path_bench -- --sample-size=60 \
    | tee target/bench-logs/bench-local-$i.log
done
```

Criterion output is stored under `target/criterion/`.

### 1.4 One-click command list (recommended)

Default one-click flow (tests + local-only benchmark + regression gate):

```bash
./scripts/bench.sh
```

Redis flow:

```bash
./scripts/bench.sh redis --redis-url redis://127.0.0.1:6379
```

Local benchmark only:

```bash
./scripts/bench.sh bench-local --runs 3 --sample-size 80
```

Export baseline (after a fresh local-only benchmark run):

```bash
./scripts/bench.sh baseline --run-bench
```

Regression gate only:

```bash
./scripts/bench.sh regression --threshold 0.10
```

---

## 2. Regression Gate

### 2.1 Export baseline

```bash
cargo run --bin export_bench_baseline --
```

Default output: `docs/benchmarks/cache_path_bench.json`

### 2.2 Threshold check

```bash
cargo run --bin check_bench_regression -- --threshold 0.15
```

- `0.15` means up to 15% slowdown is allowed.
- The checker compares current mean latency against baseline mean latency.
- Any key benchmark over threshold returns non-zero exit code (CI friendly).

### 2.3 Scripted regression check

```bash
./scripts/bench.sh regression --threshold 0.15
```

---

## 3. Full Load-Test Workflow (Recommended)

### 3.1 Stage A: correctness and environment

1. Run `cargo test`
2. Choose local-only or Redis mode
3. Confirm parameters: `sample-size`, `runs`, `threshold`

### 3.2 Stage B: smoke benchmark

Run a small sample to verify the pipeline quickly:

```bash
./scripts/bench.sh bench-local --sample-size 20
```

### 3.3 Stage C: formal benchmark

Use at least 3 runs:

```bash
./scripts/bench.sh bench-local --runs 3 --sample-size 60
```

For Redis remote path:

```bash
./scripts/bench.sh bench-redis --runs 3 --sample-size 60 --redis-url redis://127.0.0.1:6379
```

### 3.4 Stage D: update baseline

After confirming a stable version:

```bash
./scripts/bench.sh baseline --run-bench
```

### 3.5 Stage E: gate regression

```bash
./scripts/bench.sh regression --threshold 0.15
```

### 3.6 Stage F: failure handling

1. Check for host noise (CPU contention, thermal throttling, background tasks)
2. Re-run 2-3 times to verify reproducibility
3. If only remote path fails, validate local-only first
4. Use profiler to locate hotspots (allocations, lock contention, serialization, network I/O)

---

## 4. API Stability Policy

### 4.1 Backward compatibility rules

1. Public cache APIs (`get/set/del/mget/mset/mdel`) must remain signature-compatible within minor versions.
2. Published macro parameters must not be renamed; new parameters must have defaults.
3. Macro default behavior changes must be documented in changelog with migration examples.
4. Metric names under `accelerator_cache_*` should remain stable once published.

### 4.2 Breaking change template

Each breaking change should include:

1. `What changed` (interface/default/behavior)
2. `Why` (problem and expected gain)
3. `Impact` (who is affected)
4. `Migration` (step-by-step with before/after code)
5. `Rollback` (rollback trigger and strategy)

---

## 5. Recommended Project Structure and Macro Template

### 5.1 Recommended layout

```text
src/
  cache/
    user_cache.rs      # LevelCache construction and config
  repo/
    user_repo.rs       # DB query and batch query
  service/
    user_service.rs    # business methods + macro annotations
  api/
    user_handler.rs
```

### 5.2 Service-layer macro template

```rust
use accelerator::macros::{cache_evict, cache_evict_batch, cache_put, cacheable, cacheable_batch};

impl UserService {
    #[cacheable(cache = self.user_cache, key = user_id)]
    async fn get_user(&self, user_id: u64) -> CacheResult<Option<User>> {
        self.repo.get_user(user_id).await
    }

    #[cacheable_batch(cache = self.user_cache, keys = user_ids)]
    async fn batch_get_users(&self, user_ids: Vec<u64>) -> CacheResult<HashMap<u64, Option<User>>> {
        self.repo.batch_get_users(&user_ids).await
    }

    #[cache_put(cache = self.user_cache, key = user.id, value = Some(user.clone()))]
    async fn save_user(&self, user: User) -> CacheResult<()> {
        self.repo.save_user(user).await
    }

    #[cache_evict(cache = self.user_cache, key = user_id)]
    async fn delete_user(&self, user_id: u64) -> CacheResult<()> {
        self.repo.delete_user(user_id).await
    }

    #[cache_evict_batch(cache = self.user_cache, keys = user_ids)]
    async fn batch_delete_users(&self, user_ids: Vec<u64>) -> CacheResult<()> {
        self.repo.batch_delete_users(&user_ids).await
    }
}
```

---

## 6. Pre-release Checklist

1. `cargo test`
2. `./scripts/bench.sh bench-local --runs 3 --sample-size 60`
3. `./scripts/bench.sh regression --threshold 0.15`
4. Review whether `docs/cache-ops-runbook.md` needs updates
5. Update changelog with compatibility and migration notes
