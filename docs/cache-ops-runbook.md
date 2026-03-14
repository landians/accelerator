# Cache Ops Runbook

本手册用于默认后端组合（moka + redis）的线上排障，也适用于实现了等价语义的自定义后端。

## 1. Redis 不可达

### 现象

- `CacheError::Backend(...)` 持续增长。
- `accelerator_cache_remote_miss_total` 短时间异常升高。
- 应用日志出现 redis 连接/命令失败。

### 快速检查

1. 使用 `redis-cli -u <redis_url> ping` 验证基础连通。
2. 查看 `LevelCache::diagnostic_snapshot()`：
   - `remote_backend_ready` 是否为 `true`。
   - `config.mode` 是否包含 remote。
3. 检查应用指标：
   - `accelerator_cache_invalidation_publish_failures_total`
   - `accelerator_cache_load_timeout_total`

### 缓解策略

1. 临时切换为 `CacheMode::Local`（若业务可接受短期单机语义）。
2. 降低 loader 压力：缩短热点 key TTL、限制突发流量。
3. 如果开启 `stale_on_error`，确认读路径传入 `ReadOptions { allow_stale: true, .. }`。

---

## 2. 失效广播异常（Pub/Sub）

### 现象

- A 实例 `del/mdel` 后，B 实例 L1 仍读到旧值。
- 指标 `accelerator_cache_invalidation_receive_failures_total` 持续增长。

### 快速检查

1. 确认配置：`broadcast_invalidation=true` 且模式为 `Both`。
2. 查看 `diagnostic_snapshot()`：
   - `invalidation_listener_started` 是否为 `true`。
   - `invalidation_channel` 是否存在且与预期 area 一致。
3. 检查 Redis 侧 `PUBSUB CHANNELS` 与订阅链路。

### 缓解策略

1. 临时降级：读路径允许回源，保障最终一致性。
2. 手动触发批量失效（按业务 area 做 key 清理）。
3. 复盘 listener 抖动窗口，必要时提高 Redis 稳定性和网络 QoS。

---

## 3. 回源超时（Loader Timeout）

### 现象

- `CacheError::Timeout("loader")` 增多。
- `accelerator_cache_load_timeout_total` 增长明显。

### 快速检查

1. 检查 `config.loader_timeout` 是否过小。
2. 通过日志字段定位：
   - `op=get|mget`
   - `result=error`
   - `error_kind=timeout`
3. 评估 DB/下游延迟是否超过缓存层超时阈值。

### 缓解策略

1. 短期上调 `loader_timeout`，避免误超时。
2. 启用或确认 `penetration_protect=true`，防止并发击穿。
3. 结合 `refresh_ahead + stale_on_error` 提升故障窗口可用性。

---

## 4. 统一排障入口建议

建议在管理接口暴露以下只读信息：

1. `diagnostic_snapshot()` 输出（配置 + counters + runtime flags）。
2. `otel_metric_points()` 输出（便于接入 OTel 指标管道）。
3. 最近一段 `accelerator::ops` 日志（按 `area/op/key_hash/error_kind` 聚合）。

该入口应只读，不提供线上动态改配置能力。
