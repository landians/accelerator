# Performance & Engineering Playbook

本手册用于落地迭代 D，覆盖 benchmark 命令清单、完整压测流程、回归门禁、兼容策略和推荐项目结构。

相关文件：

1. 基准入口：`benches/cache_path_bench.rs`
2. 一键脚本：`scripts/bench.sh`
3. 基线导出：`src/bin/export_bench_baseline.rs`
4. 回归检测：`src/bin/check_bench_regression.rs`

## 1. 基准测试（criterion）

### 1.1 覆盖矩阵

当前 benchmark 文件：`benches/cache_path_bench.rs`。

覆盖场景：

1. 单 key 命中：`single_key/hit/manual` vs `single_key/hit/macro`
2. 单 key miss 回源：`single_key/miss/manual` vs `single_key/miss/macro`
3. 批量命中：`batch/hit/manual/32` vs `batch/hit/macro/32`
4. 批量 miss 回源：`batch/miss/manual/32` vs `batch/miss/macro/32`

补充说明：

1. 默认会先跑本地路径（`moka`）。
2. 当 `ACCELERATOR_BENCH_REDIS_URL` 可用时，还会跑远程路径（Redis）。
3. 如果 Redis 地址不可用，远程基准可能 panic；可通过 local-only 模式绕过。

### 1.2 运行前准备

建议顺序：

1. 先确保功能正确：`cargo test`
2. 压测时尽量固定机器状态（接电、关闭大负载后台任务）
3. 远程基准前确认 Redis 可用（例如 `redis-cli ping`）

### 1.3 原生命令清单（不走脚本）

单次正式基准（推荐）：

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
```

仅跑本地路径（跳过 Redis）：

```bash
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 \
cargo bench --bench cache_path_bench -- --sample-size=60
```

指定 Redis 跑远程路径：

```bash
ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:6379 \
cargo bench --bench cache_path_bench -- --sample-size=60
```

重复执行 3 次并落盘日志（排除偶发抖动）：

```bash
mkdir -p target/bench-logs
for i in 1 2 3; do
  ACCELERATOR_BENCH_REDIS_URL=redis://127.0.0.1:0 \
  cargo bench --bench cache_path_bench -- --sample-size=60 \
    | tee target/bench-logs/bench-local-$i.log
done
```

执行后，criterion 结果位于 `target/criterion/`。

### 1.4 一键脚本命令清单（推荐）

默认一键流程（测试 + local-only 基准 + 回归门禁）：

```bash
./scripts/bench.sh
```

Redis 流程：

```bash
./scripts/bench.sh redis --redis-url redis://127.0.0.1:6379
```

仅跑本地基准：

```bash
./scripts/bench.sh bench-local --runs 3 --sample-size 80
```

导出基线（先跑一轮 local-only）：

```bash
./scripts/bench.sh baseline --run-bench
```

只跑回归门禁：

```bash
./scripts/bench.sh regression --threshold 0.10
```

---

## 2. 回归门禁

### 2.1 基线导出

```bash
cargo run --bin export_bench_baseline --
```

默认输出：`docs/benchmarks/cache_path_bench.json`。

### 2.2 阈值校验

```bash
cargo run --bin check_bench_regression -- --threshold 0.15
```

- `0.15` 表示允许最多 15% 回退。
- 脚本会逐项比较当前 mean latency 和基线值。
- 任一关键项超过阈值即返回非 0（适合接入 CI）。

### 2.3 脚本化回归校验

```bash
./scripts/bench.sh regression --threshold 0.15
```

---

## 3. 完整压测流程（建议执行顺序）

### 3.1 阶段 A：功能与环境检查

1. `cargo test`
2. 选择 local-only 还是 redis 模式
3. 确认压测参数：`sample-size`、`runs`、`threshold`

### 3.2 阶段 B：冒烟压测

先跑小样本快速确认链路可用：

```bash
./scripts/bench.sh bench-local --sample-size 20
```

### 3.3 阶段 C：正式压测

建议至少 3 轮：

```bash
./scripts/bench.sh bench-local --runs 3 --sample-size 60
```

如果需要 Redis 远程路径：

```bash
./scripts/bench.sh bench-redis --runs 3 --sample-size 60 --redis-url redis://127.0.0.1:6379
```

### 3.4 阶段 D：导出或更新基线

当确认当前版本可作为新基线时执行：

```bash
./scripts/bench.sh baseline --run-bench
```

### 3.5 阶段 E：回归判定

```bash
./scripts/bench.sh regression --threshold 0.15
```

### 3.6 阶段 F：失败处理建议

1. 先看是否环境抖动（CPU 占用、温控、后台任务）。
2. 重跑 2-3 次确认是否稳定复现。
3. 如仅远程路径失败，先切 local-only 验证主逻辑改动影响。
4. 再用 profiler 定位热点函数（分配、锁竞争、序列化、网络 I/O）。

---

## 4. API 稳定策略

### 4.1 向后兼容规则

1. 对外固定 API（`get/set/del/mget/mset/mdel`）在 minor 版本内不得破坏签名。
2. 已发布宏参数不得重命名；新增参数必须有默认值。
3. 变更宏默认行为必须在 changelog 标注并给迁移示例。
4. 指标名采用稳定前缀 `accelerator_cache_*`，发布后不随意重命名。

### 4.2 破坏性变更模板

每次破坏性变更需附带以下内容：

1. `What changed`：变更点（接口/默认值/行为）。
2. `Why`：问题背景与收益。
3. `Impact`：受影响用户范围。
4. `Migration`：逐步迁移步骤（含旧/新代码对照）。
5. `Rollback`：回退方案与触发条件。

---

## 5. 推荐项目结构与宏模板

### 5.1 推荐结构

```text
src/
  cache/
    user_cache.rs      # LevelCache 构建与配置
  repo/
    user_repo.rs       # DB 查询与批量查询
  service/
    user_service.rs    # 业务方法 + 过程宏注解
  api/
    user_handler.rs
```

### 5.2 服务层宏模板

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

## 6. 发布前建议清单

1. `cargo test`
2. `./scripts/bench.sh bench-local --runs 3 --sample-size 60`
3. `./scripts/bench.sh regression --threshold 0.15`
4. 检查 `docs/cache-ops-runbook.md` 是否需要更新
5. 补齐 changelog（包含兼容性与迁移说明）
