# 文档术语基线

本术语表定义项目文档中的统一表达，用于 README、设计文档、运维手册和性能文档。

## 1. 核心领域术语

| 规范术语 | 定义 | 推荐写法 | 避免/替换 |
| --- | --- | --- | --- |
| 本地缓存（L1） | 进程内缓存层 | L1、本地层 | 内存层（当语义是缓存分层时） |
| 远程缓存（L2） | 跨实例共享缓存层 | L2、远端层 | 分布式层（若表达的是其他概念） |
| 回源加载（Miss load） | 缓存未命中后从数据源加载 | 回源、miss load | DB fallback、兜底查询 |
| 回写（Write-back） | 回源成功后将值写回缓存 | 写回、回填写入 | write-through（除非明确是直写语义） |
| 失效广播（Invalidation broadcast） | 跨实例发布缓存失效信号 | 失效 Pub/Sub、失效消息 | 缓存同步事件（语义不明确） |
| singleflight 去重 | 同 key 并发回源去重 | singleflight 去重、并发去重 | 请求合并（如果不是同 key 去重） |
| 空值缓存（Negative cache） | 缓存明确空结果（`None`/`Null`） | 空值缓存、null cache | miss 标记（容易与“未存储”混淆） |
| 提前刷新（Refresh-ahead） | 键接近过期时提前刷新 | 提前刷新、预刷新 | 异步刷新（语义过宽） |
| 陈旧值回退（Stale fallback） | 回源失败且策略允许时返回旧值 | stale 回退、stale-on-error | 降级命中（语义不明确） |

## 2. API 与运行时术语

| 规范术语 | 定义 |
| --- | --- |
| 缓存模式 | 运行模式：`Local`、`Remote`、`Both` |
| 读取选项 | 读时开关：`allow_stale`、`disable_load` |
| 运行时诊断 | `diagnostic_snapshot`、`metrics_snapshot`、`otel_metric_points` |
| 批量 API | `mget`、`mset`、`mdel`、`MLoader::mload` |

## 3. 可观测性术语

文档与排障沟通统一使用以下表达：

- 日志字段：`area`、`op`、`result`、`latency_ms`、`error_kind`
- 指标前缀：`accelerator_cache_*`
- 操作名：`get`、`mget`、`set`、`mset`、`del`、`mdel`、`warmup`

## 4. 写作规则

1. 文档首次出现建议写作 `回源加载（Miss load）`。
2. 同一章节内一个概念只使用一种规范术语。
3. 必须保留历史术语时，在首次出现处给出映射关系。
4. API 标识符统一用代码样式（例如 `MLoader::mload`）。
