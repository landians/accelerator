# Cache Ops Runbook

> Terminology baseline: see [docs/terminology.md](./terminology.md).

This runbook targets the default backend stack (`moka` + `redis`) and also applies to custom backends that preserve the same semantics.

## 1. Redis Unreachable

### Symptoms

- `CacheError::Backend(...)` keeps increasing.
- `accelerator_cache_remote_miss_total` spikes in a short window.
- Application logs show Redis connection or command failures.

### Quick Checks

1. Verify network reachability with `redis-cli -u <redis_url> ping`.
2. Inspect `LevelCache::diagnostic_snapshot()`:
   - `remote_backend_ready` should be `true`.
   - `config.mode` should include remote mode (`Remote` or `Both`).
3. Check key metrics:
   - `accelerator_cache_invalidation_publish_failures_total`
   - `accelerator_cache_load_timeout_total`

### Mitigation

1. Temporarily switch to `CacheMode::Local` if single-node semantics are acceptable.
2. Reduce miss-load pressure on source of truth (shorten hot-key TTL, throttle spikes).
3. If `stale_on_error` is enabled, confirm reads pass `ReadOptions { allow_stale: true, .. }`.

---

## 2. Invalidation Broadcast Failure (Pub/Sub)

### Symptoms

- After instance A executes `del/mdel`, instance B still serves stale L1 data.
- `accelerator_cache_invalidation_receive_failures_total` keeps growing.

### Quick Checks

1. Confirm `broadcast_invalidation=true` and mode is `Both`.
2. Inspect `diagnostic_snapshot()`:
   - `invalidation_listener_started` should be `true`.
   - `invalidation_channel` should exist and match the expected cache area.
3. Check Redis `PUBSUB CHANNELS` and subscription state.

### Mitigation

1. Temporarily degrade to miss-load reads to maintain eventual consistency.
2. Trigger manual key cleanup by cache area if needed.
3. Review listener retry windows; improve Redis/network reliability where necessary.

---

## 3. Loader Timeout

### Symptoms

- `CacheError::Timeout("loader")` increases.
- `accelerator_cache_load_timeout_total` rises noticeably.

### Quick Checks

1. Verify whether `config.loader_timeout` is too aggressive.
2. Filter logs with fields:
   - `op=get|mget`
   - `result=error`
   - `error_kind=timeout`
3. Compare downstream latency (DB/RPC) to cache loader timeout.

### Mitigation

1. Temporarily raise `loader_timeout` to avoid false timeouts.
2. Ensure `penetration_protect=true` to reduce concurrent stampede.
3. Combine `refresh_ahead` and `stale_on_error` for better fault-window availability.

---

## 4. Recommended Unified Troubleshooting Endpoint

Expose the following read-only diagnostics in an admin endpoint:

1. `diagnostic_snapshot()` output (effective config, counters, runtime flags).
2. `otel_metric_points()` output (easy handoff to OTel metric pipelines).
3. Recent `accelerator::ops` logs (aggregate by `area/op/error_kind`).

This endpoint should be read-only. Avoid live config mutation in production.
