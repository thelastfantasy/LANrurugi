# Contract: 入库可见性（Pending vs Published）

**Feature**: [spec.md](../spec.md) | **Plan**: [plan.md](../plan.md) | **Data model**: [data-model.md](../data-model.md)

本 feature 不新增端点、不改变任何既有响应结构。它给既有端点增加一条**前置状态**：当 id 是 SET `LANRURUGI_PENDING_VISIBILITY` 的成员时，该档案视为尚未发布。下面的表是该状态对客户端可观察行为的完整契约。

## 通用

- pending 判定：archive 逻辑库（DB 0）里 SET `LANRURUGI_PENDING_VISIBILITY` 的成员（2026-11-13 由单键改为 SET，见 research R8）。
- pending 与 published 的区分对客户端**不可见**：pending 档案的响应与「不存在的档案」完全一致。
- 404 响应体沿用既有 `OperationResponse` 形状：

```json
{ "operation": "<handler>", "error": "<id> does not exist.", "success": 0 }
```

- 所有「已发布」行为保持不变；本契约只约束 pending 期间。

## 搜索 / 发现入口

| 入口 | pending 行为 | published 行为 |
|------|--------------|----------------|
| `GET /api/search`（默认分组，`groupby_tanks=true`） | 不返回该 id；不因 Tankoubon 成员关系被带回；`recordsFiltered` 不把它计入 | 不变 |
| `GET /api/search?...&groupby_tanks=false` | 不返回 | 不变 |
| `GET /api/search/archives` | 不返回 | 不变 |
| `GET /api/search/ids` | 不返回 | 不变 |
| `GET /api/search/suggest` | 不返回 | 不变 |
| `GET /api/search/random` | 不返回 | 不变 |
| OPDS 搜索 | 不返回 | 不变 |
| Tankoubon 卡片 `member_matches` | pending 成员不得出现 | 不变 |

**不变量**：过滤发生在候选集组装的最后一步，位于访客 `restrict_to_archive_ids` 的 `extend` 之后；「先过滤、后并入可见 id」视为违约（D2）。

## 阅读页只读入口

| 入口 | pending 行为 | published 行为 |
|------|--------------|----------------|
| `GET /api/archives/{id}/metadata` | 404，体不含标题/标签/页数 | 不变 |
| `GET /api/archives/{id}/files` | 404，不返回页列表 | 不变 |
| `GET /api/archives/{id}/page` | 404，不返回任何字节 | 不变 |
| `GET /api/archives/{id}/page-dimensions` | 404 | 不变 |
| `GET /api/archives/{id}/thumbnail` | 404 | 不变 |
| `GET /api/archives/{id}/thumbnails` | 404 | 不变 |
| `GET /api/archives/{id}/download` | 404 | 不变 |
| `GET /api/archives/{id}/download-link` | 404 | 不变 |
| `/reader?id={id}`（前端） | metadata 404 → 现有错误分支，不渲染任何页 | 不变 |

**保留的例外**：`file` 为空时 `files`/`page` 仍返回 409「still being ingested」。那是「记录先于字节」的历史兜底（beta5 的 `defer_file_path` 顺序），本 feature 之后正常入库不再产生空 `file` 窗口，但扫描器/异常路径可能留下，保持原行为。

## 管理员枚举入口

| 入口 | pending 行为 | published 行为 |
|------|--------------|----------------|
| `GET /api/archives` | 列表中不含 pending 档案 | 不变 |
| `GET /api/archives/untagged` | 同上 | 不变 |

访客对这两个端点本来就是 403（`route_policy.csv`），所以本契约对访客无新增可见面。

## 发布语义

- 当且仅当「本次 ingest 触发的最后一个 metadata 插件返回」后，键被删除；下一个请求即可见、可读。
- 插件返回错误、没有启用插件、插件被跳过，都视为「返回」。
- 发布不重建索引、不刷新缓存；索引在 cataloguing 时已写好，门禁只是暂时把 id 从读路径上拿掉。

## 失败语义

| 情况 | 可观察结果 |
|------|------------|
| 写 pending 键失败 | 入库失败（客户端看到入库错误），不会出现「未完成但可见」 |
| 插件失败 | 档案照常发布（带已收集标签） |
| 清 pending 键失败 | 档案短暂隐藏；重试 + 启动收敛后发布；不会永久隐藏 |
| 进程崩溃在插件中途 | 重启后启动收敛清除键，档案发布（带已有标签） |
| 重复入库已发布档案 | 不写键，档案全程可见 |

## 兼容性说明

- 端点路径、方法、成功响应结构均不变。
- pending 期间由「200」变为「404」是唯一的行为变化；这些档案在完成前没有稳定契约，且此变化正是本 feature 的目的。
- 不改变扫描器 watcher / full scan 的「入库即可见」语义。
