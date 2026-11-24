# Contract: 下载/入库的追踪 span

**Status**: 本特性新增 | **Type**: 追踪数据契约（rustrak / Sentry 兼容），不改变任何 HTTP API

## 形状

一次下载任务 = 一条 trace，父事务 + 六个子 span。阶段词表与 `contracts/jobs-phases.md` 完全一致（同一份词表，不允许两套命名）。

| 角色 | `op` | `description` / 事务名 | 备注 |
|---|---|---|---|
| 父事务 | `queue.task` | `download_url` | 覆盖任务全程，成功/失败都要 `finish()` |
| 子 span | `queue.wait` | `queued` | 等插件级 job 槽 |
| 子 span | `plugin.exec` | `negotiate` | 插件 `execDownload` |
| 子 span | `http.wait` | `waiting` | 等按域并发许可 |
| 子 span | `file.transfer` | `transfer` | 字节传输（多资源合并为一个 span 或每个资源一个子 span，二选一但同一构建内保持一致） |
| 子 span | `file.ingest` | `ingest` | 打包 + 拷贝 + 落库 |
| 子 span | `plugin.metadata` | `publish` | 元数据插件链 + 发布 |

## 语义

| 规则 | 内容 |
|---|---|
| T1 | 父事务必须带上 `download.queue_item_id`、`download.plugin`（namespace）、`download.url`（去掉 query 后的来源）。 |
| T2 | 阶段悬停数据（`phases`）与 span 必须反映同一份实测时间；不允许 span 用另一套打点。 |
| T3 | 失败时父事务 `set_status(InternalError)` 并带 `error.class`；子 span 至少能定位到失败的那一段。 |
| T4 | 埋点关闭（未配置 DSN / 诊断被禁用）时，任务行为与耗时不得受影响；追踪调用不得成为失败路径。 |
| T5 | 有 HTTP 父上下文时（例如从某个请求发起的一次性下载）挂到那条 trace 下；后台任务自带独立 trace。 |

## 可验证的观测点

- rustrak 事务详情里一次 `download_url` 任务可见父事务 + 6 个子 span，且父时长 ≥ 子段之和。
- 把 DSN 清空后跑同一条下载：`phases` 与耗时与开启时一致（追踪不参与逻辑）。
