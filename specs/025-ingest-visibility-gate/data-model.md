# Phase 1 Data Model: 入库可见性门禁

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Date**: 2026-11-10

本 feature 不新增持久化实体、不改任何 Redis 记录的字段。唯一的新数据是一类**瞬态键**，它的存在与否就是「这本书现在能不能被看到」的全部状态。

## Entity: Publication Marker（发布标记）

> 2026-11-13 修订（plan Addendum 第 1 条）：由「每档案一个键」改为**一个 SET**，避免每次搜索对 DB 0 全键空间做 `KEYS`/`SCAN`。

| 属性 | 说明 |
|------|------|
| key | `LANRURUGI_PENDING_VISIBILITY`（SET），位于 **archive 逻辑库（DB 0）** |
| member | 40 位十六进制内容 id；与档案记录、搜索索引使用同一个 id |
| 操作 | `SADD`（mark）/ `SREM`（publish）/ `SISMEMBER`（is_pending）/ `SMEMBERS`（pending_ids）/ `DEL`（启动收敛） |
| TTL | 无。只能由显式 publish 或启动收敛删除 |
| cardinality | 通常 0–数个；等于「此刻正在入库且插件未跑完」的档案数 |
| 遗留键 | beta5 之后写入的 `LANRURUGI_PENDING_VISIBILITY:<id>`（DB 2）从未生效；启动收敛时对 DB 0、DB 2 各 `SCAN` 一次清掉 `LANRURUGI_PENDING_VISIBILITY:*` |

pending 开始时间不再存储（原 value 只用于排查），改由 FR-014 的 mark/publish 日志提供。

衍生状态（不单独存储）：

- **Pending**：id 是该 SET 的成员。
- **Published**：id 不在 SET 中。包括从未进过 pending 的历史档案。

## State Transitions

```text
                 mark_pending(id)                 publish(id)
  (no marker) ────────────────────▶ Pending ────────────────────▶ Published
       ▲                                │
       │        cataloguing/IO 失败      │
       └──────── publish(id) ◀──────────┘
                                        │
                    启动收敛 clear_all_pending()
       ◀────────────────────────────────┘
```

| 转换 | 触发 | 前置条件 | 失败处理 |
|------|------|----------|----------|
| `mark` | `catalogue_staged_file` 在拿到 content lock、确认不是 `Unchanged` 之后、rename 进最终路径之前 | id 已由 `size_aware_id` 算出（已是成员时 `SADD` 无副作用） | 返回错误，整次入库失败（fail closed） |
| `publish` | 本次 ingest 触发的最后一个 metadata 插件返回之后；或 cataloguing/IO 失败；或 `Rejected` | id 是成员 | 重试 3 次（短退避）；仍失败则 error 日志，等启动收敛 |
| `clear_all` | 服务启动，`repair_zombie_archives` 之后、队列恢复之前 | 无 | `DEL` 失败只记日志并继续启动；遗留单键逐个删除，失败同样只记日志 |

## Per-path Lifecycle

| 入库路径 | 谁 mark | 谁 publish | 备注 |
|----------|---------|------------|------|
| 下载（`ingest_downloaded_file` → `catalogue_staged_file`） | `catalogue_staged_file` | 同一函数，插件返回后 | 主场景 |
| 上传（`upload.rs`） | 同一函数 | `upload.rs` 在第二次插件 pass 后（`defer_publication=true`） | 覆盖 D7 |
| 共享导入（`share_net/import.rs`） | 同一函数 | 同一函数，插件返回后 | 与下载一致 |
| 订阅自动下载 | 走下载路径 | 下载路径 | 订阅 runner 的再增强调用只针对已发布档案，不 mark（FR-010） |
| 旧文件名冲突 resolve（`resolve_overwrite`/`resolve_rename`） | 同一函数 | 同一函数 | `defer=false` |
| 扫描器 watcher / full scan | 不 mark | — | 不跑 metadata 插件，维持既有「入库即可见」；spec 已列为 out of scope |

## Invariants

- **I1（隐藏）**：只要 id 在 SET 中，该 id 不得出现在任何搜索/枚举结果中，也不得通过阅读页只读接口返回内容（404）。
- **I2（先标记后索引）**：`mark` 必须先于任何把该 id 写进搜索索引/档案记录的动作。
- **I3（发布时机）**：`publish` 只能发生在「本次 ingest 触发的所有 metadata 插件 pass 都已返回」之后；插件失败也算返回。
- **I4（只隐藏新档案）**：只对本次 ingest 新建/重键的 id 写标记；已发布档案不因重复下载、重复入库、订阅再增强而被隐藏。
- **I5（同一可见性域）**：写标记与读标记必须使用同一个 Redis 逻辑库（DB 0），不得跨库。
- **I6（无计时过期）**：键不得设置 TTL；可见性只能由显式动作改变。
- **I7（有界恢复）**：进程崩溃后，启动收敛必须在 60 秒内清除所有残留键，不留下永久隐藏的档案。
- **I8（失败闭合）**：写标记失败不得被当作「没标记也算完成」而继续入库。

## What Is Deliberately Not Modeled

- **插件进度/阶段**：门禁只关心「跑完没有」，不暴露「正在跑第几个插件」；需求是「不出现」，不是「灰显进度」。
- **失败原因**：键的值只有时间戳；插件失败不会把档案留在 pending（I3），所以不需要失败状态。
- **多实例租约/所有权**：单实例部署假设；若未来多实例，需要把「谁负责发布」升级为带租约的状态，本 feature 不做。
- **历史 pending 键的迁移**：旧实现写在 DB 2 的单键从未生效，不迁移，启动收敛时删除即可（见上表「遗留键」）。
