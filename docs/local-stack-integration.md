# Local Stack Integration Guide

> Terminology baseline: see [docs/terminology.md](./terminology.md).

This guide explains how to run the full local dependency stack (Redis + Postgres + ClickStack)
and validate cache behavior against real middleware.

## 0. One-command Checklist (from repo root)

```bash
# start stack
docker compose -f scripts/docker-compose.yml up -d

# verify container status and health
docker compose -f scripts/docker-compose.yml ps

# run key integration tests
cargo test --test redis_integration
cargo test --test stack_integration

# stop stack, keep volumes
docker compose -f scripts/docker-compose.yml down

# stop stack and remove volumes (full reset)
docker compose -f scripts/docker-compose.yml down -v
```

## 1. Start Local Environment

Compose file: `scripts/docker-compose.yml`

```bash
cd scripts
docker compose up -d
```

Default ports:

- Redis: `127.0.0.1:6379`
- Postgres: `127.0.0.1:5432`
- ClickStack UI: `127.0.0.1:8080`
- ClickStack OTLP ingest: `127.0.0.1:4317` (gRPC), `127.0.0.1:4318` (HTTP)
- ClickHouse SQL/Native (optional debugging): `127.0.0.1:8123`, `127.0.0.1:9000`

## 2. Integration Tests

### 2.1 Redis integration test

```bash
cargo test --test redis_integration
```

Optional environment variable:

```bash
export ACCELERATOR_TEST_REDIS_URL=redis://127.0.0.1:6379
```

### 2.2 Redis + Postgres integration test

```bash
cargo test --test stack_integration
```

Optional environment variables:

```bash
export ACCELERATOR_TEST_REDIS_URL=redis://127.0.0.1:6379
export ACCELERATOR_TEST_POSTGRES_DSN="postgres://accelerator:accelerator@127.0.0.1:5432/accelerator"
```

Coverage points:

1. Miss load (source-of-truth load) via `sqlx` + Postgres (single key and batch)
2. Cache hit path prevents repeated miss loads
3. `mget` miss path loads and writes back
4. `otel_metric_points` export verification

## 3. Observability in ClickStack

1. Configure application-side OTLP exporter:
   - gRPC endpoint: `http://127.0.0.1:4317`
   - HTTP endpoint: `http://127.0.0.1:4318`
2. Open ClickStack UI: `http://127.0.0.1:8080`
3. In Traces view, filter by `service.name` or span names such as `cache.get`
4. In Logs/Metrics, aggregate by fields like `target=accelerator::ops`, `area`, `error_kind`

Notes:

- The local stack is standardized on ClickStack for local OTel ingestion and storage.
- Application exporter initialization is still required on the host application side.
- By default, ClickStack OTLP endpoints require an authorization header.

### 3.1 Reusable telemetry initialization (recommended)

The repository provides reusable helpers behind feature `otlp`:

- `accelerator::telemetry::OtlpTelemetryBuilder`
- `accelerator::telemetry::OtlpTransport`
- `accelerator::telemetry::TelemetryGuard`

Minimal setup:

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

// app runs...

guard.shutdown();
```

Runnable example:

```bash
ACCELERATOR_CLICKSTACK_AUTHORIZATION="<YOUR_INGESTION_KEY>" \
cargo run --features otlp --example clickstack_otlp
```

Optional environment variables used by the example:

- `ACCELERATOR_CLICKSTACK_OTLP_ENDPOINT` (default `http://127.0.0.1:4317`)
- `ACCELERATOR_CLICKSTACK_OTLP_TRANSPORT` (`grpc` or `http`, default `grpc`)
- `ACCELERATOR_CLICKSTACK_AUTHORIZATION` (mapped to `.authorization(...)`)

## 4. Stop and Cleanup

```bash
cd scripts
docker compose down
```

To remove persistent volumes:

```bash
docker compose down -v
```
