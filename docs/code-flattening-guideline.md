# Rust Code Flattening Guideline (General)

> Terminology baseline: see [docs/terminology.md](./terminology.md).

## 1. Goal and Scope

This guideline standardizes flattening refactors in Rust projects to reduce nesting, improve readability, and preserve behavior.

Applies to:

1. Business logic (sync and async)
2. Data access layers (DB, cache, external APIs)
3. Infrastructure code (schedulers, consumers, telemetry paths)
4. Unit and integration tests

Non-goals:

1. Do not trade correctness for flatter control flow.
2. Do not over-split straightforward logic into too many tiny helpers.

---

## 2. Core Principles

### 2.1 Prefer guard clauses first

Handle invalid or blocking conditions early and return immediately.
This avoids wrapping the main path in deep `if` or `match` pyramids.

### 2.2 Keep orchestration in the main function, move detail to helpers

When one function includes validation, loading, transformation, writes, and observability, keep only stage ordering in the main function and move detail into clearly named helpers.

### 2.3 Use `continue` or `break` in loops to reduce reverse indentation

For batch loops, skip irrelevant items early and keep the happy path at the bottom of the loop.

### 2.4 Use Rust-native early-exit tools

Prefer `?`, `let-else`, and small combinators (`map`, `and_then`) over repetitive error matching boilerplate.

### 2.5 Name complex expressions before calling

Build semantic variables first (for example `payload`, `entry`, `deadline`) and then pass them into function calls. This improves readability and debugging.

### 2.6 Preserve batch semantics with batch APIs

Methods exposing batch semantics (`m*`/`batch_*`) should not degrade to N single remote calls unless the backend has no batch capability (and this is documented).

### 2.7 Centralize observability

Place metrics/logging in stable collection points whenever possible, instead of interleaving observability logic across every branch.

---

## 3. Common Refactor Patterns (With Examples)

### 3.1 Pattern A: Nested `if` -> Guard return

Before:

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

After:

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

### 3.2 Pattern B: Deep `Option` nesting -> `?` / `let-else`

Before:

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

After:

```rust
fn find_name(user: Option<User>) -> Option<String> {
    let u = user?;
    let profile = u.profile?;
    profile.name
}
```

### 3.3 Pattern C: Loop pyramid -> `continue`

Before:

```rust
for item in items {
    if item.enabled {
        if item.score > 60 {
            process(item)?;
        }
    }
}
```

After:

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

### 3.4 Pattern D: Overloaded function -> staged helpers

Before (sketch):

```rust
async fn handle(req: Request) -> Result<Response, Error> {
    // validation + query + transform + audit + response mapping in one place
}
```

After (sketch):

```rust
async fn handle(req: Request) -> Result<Response, Error> {
    let cmd = validate(req)?;
    let row = fetch_row(&cmd).await?;
    let output = compute(row)?;
    write_audit(&output).await?;
    Ok(to_response(output))
}
```

### 3.5 Pattern E: Async nested `if let` -> early returns

Before:

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

After:

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

## 4. Recommended Refactor Workflow

1. List execution stages (`validate -> load -> transform -> persist -> observe`).
2. Identify early exit conditions and convert them into guard clauses.
3. Replace loop `else` blocks with `continue`/`break` where possible.
4. Split long functions into 2-5 semantically named helpers.
5. Consolidate observability in stable collection points.
6. Re-run tests and compare behavior, especially error and edge paths.

---

## 5. Code Review Checklist

1. Is the main execution path obvious at first glance?
2. Are invalid branches returned early?
3. Is there avoidable nesting deeper than 3 levels?
4. Are loop branches using `continue` where appropriate?
5. Are batch APIs accidentally implemented as single-call loops?
6. Is error handling consistent and idiomatic (`?` where suitable)?
7. Is observability centralized rather than scattered?

---

## 6. Test Flattening Guidance

1. Keep one primary behavior assertion per test.
2. Use a linear `arrange -> act -> assert` structure.
3. Reuse helpers to keep test bodies short.
4. For external dependencies (DB/Redis/HTTP), use "skip when unavailable" policy for local developer reliability.

---

## 7. Anti-patterns and Boundaries

Avoid:

1. Splitting logic into many 1-2 line helpers only for style.
2. Over-compressing code into hard-to-read combinator chains.
3. Refactoring structure and semantics at the same time without behavior verification.

Boundary notes:

1. Flattening is not the goal; maintainability and correctness are.
2. For performance-sensitive paths, validate with benchmarks after refactor.
