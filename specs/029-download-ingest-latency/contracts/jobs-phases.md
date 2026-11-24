# Contract: `/api/jobs` 的分阶段计时（追加字段）

**Status**: 本特性新增 | **Type**: 既有响应追加可选字段（Principle II：只增不改）

## 形状

`GET /api/jobs`（以及 `/api/jobs/{id}`、`/api/minion/{id}` 若共用同一 `JobStatus` 序列化）中每个 job 对象可带一个 `phases` 数组：

```json
{
  "id": "5c0f5a0e-1f1e-4a2a-9f2c-8a0f8f3d8f4e",
  "name": "download_url",
  "state": "active",
  "progress": 0.61,
  "downloaded_bytes": 557000000,
  "total_bytes": 916072365,
  "average_bytes_per_sec": 3430804,
  "started_at": 1795482741675,
  "finished_at": null,
  "phases": [
    { "name": "queued",    "started_at": 1795482741675, "ended_at": 1795483420000 },
    { "name": "negotiate", "started_at": 1795483420000, "ended_at": 1795483512000 },
    { "name": "waiting",   "started_at": 1795483512000, "ended_at": 1795483524000 },
    { "name": "transfer",  "started_at": 1795483524000, "ended_at": null }
  ],
  "result": null,
  "error": null
}
```

## 语义

| 规则 | 内容 |
|---|---|
| C1 | `phases` 是**可选**字段：为空时整个键省略（不是 `[]`、不是 `null`）。既有客户端在缺少它时必须保持今天的行为。 |
| C2 | `name` 只能取自固定词表：`queued` / `negotiate` / `waiting` / `transfer` / `ingest` / `publish`。新增阶段名属于破坏性变更，需要新版本字段。 |
| C3 | 顺序单调：数组顺序即发生顺序；后一项的 `started_at` 不得早于前一项的 `ended_at`。 |
| C4 | 至多一项 `ended_at == null`（正在进行的那一段）。任务进入 `finished`/`failed` 后，数组中所有项都必须有 `ended_at`。 |
| C5 | 时间单位是 epoch 毫秒，与 `started_at`/`finished_at` 一致。 |
| C6 | 未走到的阶段不出现（例如任务在 `negotiate` 就失败，则没有 `transfer`/`ingest`/`publish` 条目）。 |
| C7 | 不落盘：服务重启后 job 与 `phases` 一起消失。消费方不得假定已完成的 job 在重启后仍可查。 |

## 与既有字段的关系

- `started_at` ≤ 第一个阶段的 `started_at`；`finished_at` ≥ 最后一个阶段的 `ended_at`。两者之间的差额是**未归属间隙**，消费方不得把它算进任何一段。
- `average_bytes_per_sec` 仍只覆盖真正有字节流动的窗口（既有语义，见 `jobs.rs:60-77` 的注释）——它**不是**端到端速率，`phases` 也不改变这一点。UI 必须把这一限制呈现出来（例如“仅传输”）。

## 兼容性

- 新增字段、只增不改：`GET /api/jobs` 的既有字段一个都不变。
- 旧服务（无 `phases`）→ 新前端：退回单段展示。
- 新服务（有 `phases`）→ 旧客户端：忽略该键。
- 文档站由 `lanrurugi-api-docgen` 从源码路由表生成，`phases` 需在对应 sidecar 片段里补文档（见 tasks）。
