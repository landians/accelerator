# accelerator
Accelerator is a universal data access acceleration component, designed to enhance the data reading performance and system stability in high-concurrency scenarios.

## Fixed Backend Best Practice

- Runtime is fixed to `moka` (L1) + `redis` (L2).
- L1 uses `moka`'s built-in eviction behavior; there is no user-facing `EvictionPolicy` switch.
- Enhanced capabilities are built-in: `warmup`, `refresh_ahead`, `stale_on_error`, and Redis Pub/Sub invalidation broadcast.
- Production-style usage example: `examples/fixed_backend_best_practice.rs`.
- Run it with:

```bash
cargo run --example fixed_backend_best_practice
```

If Redis is not reachable at `ACCELERATOR_REDIS_URL` (default `redis://127.0.0.1:6379`), the example exits gracefully.

## Performance Baseline & Regression Gate

Iteration D includes a Criterion benchmark suite and a regression gate:

- Benchmark file: `benches/cache_path_bench.rs`
- Baseline snapshot: `docs/benchmarks/cache_path_bench.json`
- Gate tools (Rust binaries):
  - `src/bin/export_bench_baseline.rs`
  - `src/bin/check_bench_regression.rs`

Quick start:

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
cargo run --bin check_bench_regression -- --threshold 0.15
```

Detailed engineering guide: `docs/performance-engineering-playbook.md`.

## Redis Integration Tests

Integration tests that require a real Redis are in `tests/redis_integration.rs`.

- They run automatically in `cargo test`.
- Each test first probes Redis and will skip itself when Redis is unavailable.
- You can override Redis endpoint with `ACCELERATOR_TEST_REDIS_URL`.

## Full Local Stack (Redis + Postgres + Prometheus + Grafana + OTel + Tempo)

The repository provides a ready-to-run docker compose stack:

```bash
cd scripts
docker compose up -d
```

Then you can run end-to-end integration tests:

```bash
cargo test --test redis_integration
cargo test --test stack_integration
```

`stack_integration` uses a real `sqlx + Postgres` loader path.

Detailed setup and tracing notes: `docs/local-stack-integration.md`.
