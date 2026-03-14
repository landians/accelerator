# Documentation Terminology Baseline

This glossary defines canonical terms used across project documentation.
Use these terms consistently in README, design docs, runbooks, and benchmarks.

## 1. Core Domain Terms

| Canonical Term | Definition | Preferred Variants | Avoid / Replace |
| --- | --- | --- | --- |
| Local cache (L1) | Process-local cache layer | L1, local layer | in-memory layer (when referring to cache tier semantics) |
| Remote cache (L2) | Shared cross-instance cache layer | L2, remote layer | distributed cache layer (if used as a different concept) |
| Miss load (source-of-truth load) | Load from source of truth on cache miss | miss load, source-of-truth load | DB fallback, fallback query |
| Write-back | Write loaded value back to cache after load | cache write-back, backfill write | write through (unless write-through semantics are meant) |
| Invalidation broadcast | Cross-instance invalidation signal publication | invalidation Pub/Sub, invalidation message | cache sync event (ambiguous) |
| Singleflight deduplication | Deduplicate concurrent loads for same key | singleflight dedup, miss dedup | request merge (if not same-key dedup) |
| Negative cache | Cache explicit empty result (`None`/`Null`) | null cache, empty-value cache | miss marker (if it implies no stored entry) |
| Refresh-ahead | Refresh near-expiration entries proactively | proactive refresh | async refresh (too broad) |
| Stale fallback | Return stale value when load fails and policy allows | stale-on-error fallback | degraded hit (ambiguous) |

## 2. API and Runtime Terms

| Canonical Term | Definition |
| --- | --- |
| Cache mode | Runtime mode: `Local`, `Remote`, `Both` |
| Read options | Read-time switches: `allow_stale`, `disable_load` |
| Runtime diagnostics | `diagnostic_snapshot`, `metrics_snapshot`, `otel_metric_points` |
| Batch APIs | `mget`, `mset`, `mdel`, `MLoader::mload` |

## 3. Observability Terms

Use the same naming in docs and operational communication:

- log fields: `area`, `op`, `result`, `latency_ms`, `error_kind`
- metric prefix: `accelerator_cache_*`
- operation names: `get`, `mget`, `set`, `mset`, `del`, `mdel`, `warmup`

## 4. Writing Rules

1. On first mention in a document, prefer `Miss load (source-of-truth load)` style.
2. Use one canonical term per concept inside the same section.
3. If a legacy term must appear, provide canonical mapping once in parentheses.
4. Keep API identifiers in code form (for example `MLoader::mload`).
