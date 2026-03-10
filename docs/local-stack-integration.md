# Local Stack Integration Guide

本文档说明如何在本地启动完整依赖环境（Redis + Postgres + ClickStack），并运行集成测试验证缓存组件在真实中间件下的行为。

## 0. 一键命令清单（推荐从仓库根目录执行）

```bash
# 一键启动
docker compose -f scripts/docker-compose.yml up -d

# 一键验证（看容器状态/健康）
docker compose -f scripts/docker-compose.yml ps

# 一键验证（跑关键集成测试）
cargo test --test redis_integration
cargo test --test stack_integration

# 一键清理（保留卷数据）
docker compose -f scripts/docker-compose.yml down

# 一键清理（删除卷数据，彻底重置）
docker compose -f scripts/docker-compose.yml down -v
```

## 1. 启动本地环境

目录：`scripts/docker-compose.yml`

```bash
cd scripts
docker compose up -d
```

默认服务端口：

- Redis: `127.0.0.1:6379`
- Postgres: `127.0.0.1:5432`
- ClickStack UI: `127.0.0.1:8080`
- ClickStack OTLP ingest: `127.0.0.1:4317`(gRPC), `127.0.0.1:4318`(HTTP)
- ClickHouse SQL/Native（可选调试）: `127.0.0.1:8123`, `127.0.0.1:9000`

## 2. 集成测试

### 2.1 Redis 集成测试（已有）

```bash
cargo test --test redis_integration
```

环境变量（可选）：

```bash
export ACCELERATOR_TEST_REDIS_URL=redis://127.0.0.1:6379
```

### 2.2 Redis + Postgres 联合集成测试（新增）

```bash
cargo test --test stack_integration
```

环境变量（可选）：

```bash
export ACCELERATOR_TEST_REDIS_URL=redis://127.0.0.1:6379
export ACCELERATOR_TEST_POSTGRES_DSN="postgres://accelerator:accelerator@127.0.0.1:5432/accelerator"
```

测试覆盖点：

1. 使用 `sqlx` + Postgres Loader 回源（单 key 与批量）。
2. 缓存命中后避免重复回源。
3. `mget` 在 miss 场景回源并回写缓存。
4. 导出 `prometheus_metrics` 与 `otel_metric_points` 验证可观测输出。

## 3. ClickStack 查看链路与指标

1. 在业务应用中接入 OTLP exporter，端点指向：
   - gRPC: `http://127.0.0.1:4317`
   - HTTP: `http://127.0.0.1:4318`
2. 打开 ClickStack UI：`http://127.0.0.1:8080`。
3. 在 Traces 视图按 `service.name` 或 span 名（如 `cache.get`）过滤。
4. 在 Logs/Metrics 视图按 `target=accelerator::ops`、`area`、`error_kind` 聚合排障。

> 当前仓库本地栈已收敛到 ClickStack（内置 OTel 采集与存储）；应用侧 exporter 仍需在宿主应用初始化。
> 默认情况下，ClickStack OTLP 入口需要认证头。推荐通过
> `OtlpTelemetryBuilder::authorization(...)` 显式配置。

### 3.1 组件化初始化（推荐）

仓库内提供了可复用初始化组件（feature: `otlp`）：

- `accelerator::telemetry::OtlpTelemetryBuilder`
- `accelerator::telemetry::OtlpTransport`
- `accelerator::telemetry::TelemetryGuard`

最小接入示例：

```rust
use accelerator::telemetry::{OtlpTelemetryBuilder, OtlpTransport};

let guard = OtlpTelemetryBuilder::new("your-service-name")
    .service_namespace("your-system")
    .service_version("1.0.0")
    .endpoint("http://127.0.0.1:4317")
    .transport(OtlpTransport::Grpc)
    .authorization("your-ingestion-key")
    .env_filter("info,accelerator=info")
    .install()?;

// 业务运行...

guard.shutdown();
```

可直接运行仓库示例验证：

```bash
ACCELERATOR_CLICKSTACK_AUTHORIZATION="<YOUR_INGESTION_KEY>" \
cargo run --features otlp --example clickstack_otlp
```

示例可选环境变量：

- `ACCELERATOR_CLICKSTACK_OTLP_ENDPOINT`（默认 `http://127.0.0.1:4317`）
- `ACCELERATOR_CLICKSTACK_OTLP_TRANSPORT`（`grpc` / `http`，默认 `grpc`）
- `ACCELERATOR_CLICKSTACK_AUTHORIZATION`（示例中映射到 `.authorization(...)`）

## 4. 关闭环境

```bash
cd scripts
docker compose down
```

如需清理持久卷：

```bash
docker compose down -v
```
