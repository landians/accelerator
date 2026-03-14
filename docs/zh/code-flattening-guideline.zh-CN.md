# Rust 代码扁平化重构规范（通用）

> 术语基线：参见 [docs/zh/terminology.zh-CN.md](./terminology.zh-CN.md)。

## 1. 目标与适用范围

本规范用于指导 Rust 项目中的“降嵌套、提可读、保语义”重构，适用于：

1. 业务逻辑（同步/异步函数）。
2. 数据访问层（数据库、缓存、外部 API）。
3. 基础设施代码（任务调度、消息消费、观测埋点）。
4. 单元测试与集成测试。

核心目标：

1. 主路径线性可读。
2. 失败路径提前退出。
3. 减少条件嵌套层级。
4. 在不改变行为的前提下提升维护效率。

---

## 2. 设计原则

### 2.1 Guard Clause 优先（先处理不可继续分支）

把错误、空输入、权限不足、依赖不可用等“阻断条件”放在函数前部并 `return`，避免主流程被包在多层 `if/else` 中。

### 2.2 主流程只保留步骤编排，细节下沉到 helper

当一个函数同时包含“验证、查询、转换、写回、埋点”等多段逻辑时，主函数只保留阶段顺序，细节移动到命名清晰的私有函数。

### 2.3 循环中优先 `continue`/`break`，避免反向缩进

在批处理或扫描逻辑中，把“不处理项”尽早 `continue`，把“正常处理路径”放在循环底部。

### 2.4 优先 `?` 传播错误，减少 `match Err(_)` 样板

Rust 的 `Result`/`Option` 已经提供扁平化工具：`?`、`let-else`、`map`、`and_then`。  
优先用语言特性表达“失败即返回”。

### 2.5 表达式先命名，再调用

复杂调用前，先构造语义变量（如 `request`、`payload`、`entry`、`deadline`），再传入函数，避免“嵌套构造 + 调用”一行过长、难调试。

### 2.6 批量语义必须走批量接口

对外暴露 `m*`/`batch_*` 语义的方法，不应在编排层退化为 N 次单条远程调用（除非后端无批量能力且已注释说明）。

### 2.7 观测逻辑集中收口

指标、日志、trace 字段尽量放在统一收口点，避免“业务代码 + 埋点代码”交叉穿插导致层级膨胀。

---

## 3. 常用重构模式（含示例）

### 3.1 模式 A：嵌套 `if` -> Guard Return

重构前：

```rust
fn parse_user_id(input: &str) -> Result<u64, String> {
    if !input.is_empty() {
        if input.chars().all(|c| c.is_ascii_digit()) {
            return input.parse::<u64>().map_err(|e| e.to_string());
        } else {
            return Err("contains non-digit".to_string());
        }
    } else {
        return Err("empty input".to_string());
    }
}
```

重构后：

```rust
fn parse_user_id(input: &str) -> Result<u64, String> {
    if input.is_empty() {
        return Err("empty input".to_string());
    }

    if !input.chars().all(|c| c.is_ascii_digit()) {
        return Err("contains non-digit".to_string());
    }

    input.parse::<u64>().map_err(|e| e.to_string())
}
```

---

### 3.2 模式 B：`match Option` 深嵌套 -> `let-else`

重构前：

```rust
fn find_name(user: Option<User>) -> Option<String> {
    if let Some(u) = user {
        if let Some(profile) = u.profile {
            if let Some(name) = profile.name {
                return Some(name);
            }
        }
    }
    None
}
```

重构后：

```rust
fn find_name(user: Option<User>) -> Option<String> {
    let u = user?;
    let profile = u.profile?;
    profile.name
}
```

---

### 3.3 模式 C：循环 `if/else` 金字塔 -> `continue`

重构前：

```rust
for item in items {
    if item.enabled {
        if item.score > 60 {
            process(item)?;
        }
    }
}
```

重构后：

```rust
for item in items {
    if !item.enabled {
        continue;
    }
    if item.score <= 60 {
        continue;
    }
    process(item)?;
}
```

---

### 3.4 模式 D：单函数过载 -> 阶段化 helper

重构前（示意）：

```rust
async fn handle(req: Request) -> Result<Response, Error> {
    // 参数校验 + DB 查询 + 业务计算 + 写审计日志 + 组装响应
    // 全部堆在一个函数里
}
```

重构后（示意）：

```rust
async fn handle(req: Request) -> Result<Response, Error> {
    let cmd = validate(req)?;
    let row = fetch_row(&cmd).await?;
    let output = compute(row)?;
    write_audit(&output).await?;
    Ok(to_response(output))
}
```

---

### 3.5 模式 E：异步 `if let` 多层 -> 早返回 + 小函数

重构前：

```rust
async fn maybe_send(msg: Option<Message>, client: &Client) -> Result<(), Error> {
    if let Some(m) = msg {
        if m.should_send {
            client.send(m).await?;
        }
    }
    Ok(())
}
```

重构后：

```rust
async fn maybe_send(msg: Option<Message>, client: &Client) -> Result<(), Error> {
    let Some(m) = msg else { return Ok(()); };
    if !m.should_send {
        return Ok(());
    }
    client.send(m).await
}
```

---

## 4. 推荐重构流程

1. 先写“阶段清单”（validate -> load -> transform -> persist -> observe）。
2. 标出每个阶段的失败条件，尽量前置为 guard return。
3. 减少循环中的 `else` 块，改用 `continue/break`。
4. 将长函数拆分为 2-5 个语义明确的 helper。
5. 将观测代码收口（入口打开始时间，出口统一记录结果）。
6. 跑测试并做行为比对（尤其错误路径和边界输入）。

---

## 5. Code Review Checklist（通用）

1. 函数主路径是否一眼看出执行顺序？
2. 是否存在可提前返回的无效分支？
3. 是否有超过 3 层的条件嵌套可以下沉？
4. 循环里是否有可以 `continue` 的分支仍写成 `else`？
5. 是否错误地把批量语义实现成单条远程循环？
6. 错误处理是否统一、是否充分使用 `?`？
7. 日志/指标是否集中而非散落在每个分支？

---

## 6. 测试代码扁平化建议

1. 一个测试聚焦一个行为断言。
2. 结构保持 `arrange -> act -> assert`，避免条件分支污染断言。
3. 复用测试 helper 构造输入，避免测试本体过长。
4. 对外部依赖（DB/Redis/HTTP）使用“不可用则跳过”策略，保证本地可执行性。

---

## 7. 反例与边界

应避免：

1. 为了“扁平”把逻辑拆成大量 1-2 行函数，导致跳转阅读困难。
2. 用链式函数式调用过度压缩可读性（`map/and_then` 链过长）。
3. 在未验证行为一致前进行“结构 + 语义”双重改造。

边界说明：

1. 扁平化不是目标本身；可维护性和正确性优先。
2. 对性能敏感路径，要在重构后用 benchmark 验证。
