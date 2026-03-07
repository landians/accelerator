# Local Stack Integration Guide

本文档说明如何在本地启动完整依赖环境，并运行集成测试验证缓存组件在真实中间件下的行为。

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
- Prometheus: `127.0.0.1:9090`
- Grafana: `127.0.0.1:3000`（admin/admin）
- OTel Collector: `127.0.0.1:4317`(gRPC), `127.0.0.1:4318`(HTTP)
- Tempo: `127.0.0.1:3200`

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

## 3. Grafana 查看链路

1. 在业务应用中接入 OTLP tracing exporter，指向：`http://127.0.0.1:4317`。
2. 打开 Grafana Explore，选择 `Tempo` 数据源。
3. 使用 `service.name` 或 span 名（如 `cache.get`）查询。

> 当前仓库提供 Collector + Tempo + Grafana 侧环境；应用侧 exporter 需在宿主应用初始化。

## 4. 关闭环境

```bash
cd scripts
docker compose down
```

如需清理持久卷：

```bash
docker compose down -v
```
