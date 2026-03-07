# Performance & Engineering Playbook

本手册用于落地迭代 D，覆盖 benchmark、回归门禁、兼容策略和推荐项目结构。

## 1. 基准测试（criterion）

### 1.1 覆盖矩阵

当前 benchmark 文件：`benches/cache_path_bench.rs`。

覆盖场景：

1. 单 key 命中：`single_key/hit/manual` vs `single_key/hit/macro`
2. 单 key miss 回源：`single_key/miss/manual` vs `single_key/miss/macro`
3. 批量命中：`batch/hit/manual/32` vs `batch/hit/macro/32`
4. 批量 miss 回源：`batch/miss/manual/32` vs `batch/miss/macro/32`

> 当前 benchmark 默认走本地固定后端（moka），用于观察宏展开路径和手写路径的相对开销。

### 1.2 运行方式

```bash
cargo bench --bench cache_path_bench -- --sample-size=60
```

执行后，criterion 结果在 `target/criterion/` 下。

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

---

## 3. API 稳定策略

### 3.1 向后兼容规则

1. 对外固定 API（`get/set/del/mget/mset/mdel`）在 minor 版本内不得破坏签名。
2. 已发布宏参数不得重命名；新增参数必须有默认值。
3. 变更宏默认行为必须在 changelog 标注并给迁移示例。
4. 指标名采用稳定前缀 `accelerator_cache_*`，发布后不随意重命名。

### 3.2 破坏性变更模板

每次破坏性变更需附带以下内容：

1. `What changed`：变更点（接口/默认值/行为）。
2. `Why`：问题背景与收益。
3. `Impact`：受影响用户范围。
4. `Migration`：逐步迁移步骤（含旧/新代码对照）。
5. `Rollback`：回退方案与触发条件。

---

## 4. 推荐项目结构与宏模板

### 4.1 推荐结构

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

### 4.2 服务层宏模板

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

## 5. 发布前建议清单

1. `cargo test`
2. `cargo bench --bench cache_path_bench -- --sample-size=60`
3. `cargo run --bin check_bench_regression -- --threshold 0.15`
4. 检查 `docs/cache-ops-runbook.md` 是否需要更新
5. 补齐 changelog（包含兼容性与迁移说明）
