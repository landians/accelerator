# accelerator
Accelerator is a universal data access acceleration component, designed to enhance the data reading performance and system stability in high-concurrency scenarios.

## Fixed Backend Best Practice

- Runtime is fixed to `moka` (L1) + `redis` (L2).
- L1 uses `moka`'s built-in eviction behavior; there is no user-facing `EvictionPolicy` switch.
- Production-style usage example: `examples/fixed_backend_best_practice.rs`.
- Run it with:

```bash
cargo run --example fixed_backend_best_practice
```

If Redis is not reachable at `ACCELERATOR_REDIS_URL` (default `redis://127.0.0.1:6379`), the example exits gracefully.

## Redis Integration Tests

Integration tests that require a real Redis are in `tests/redis_integration.rs`.

- They run automatically in `cargo test`.
- Each test first probes Redis and will skip itself when Redis is unavailable.
- You can override Redis endpoint with `ACCELERATOR_TEST_REDIS_URL`.
