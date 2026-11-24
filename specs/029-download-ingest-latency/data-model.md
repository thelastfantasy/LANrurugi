# Data Model: 下载 → 入库的耗时构成与插件发现开销

**Feature**: `specs/029-download-ingest-latency` | **Date**: 2026-11-24

本特性不新增持久化结构。下面三个实体里，两个是**进程内**的运行时视图（随 job / 插件池生命期存在），一个是**已持久化的队列项上的一个可选追加字段**。

---

## E1. 插件声明缓存条目（PluginDescriptorCacheEntry，进程内）

一个插件对其身份与需求的声明，外加“这份声明是在哪个文件版本上取得的”。

| 字段 | 类型 | 说明 | 约束 |
|---|---|---|---|
| `namespace` | string | 由安装路径推导的命名空间（`metadata/chaika`） | 与缓存键一致；`is_safe_namespace` 已通过 |
| `module_path` | path | `.ts` 的绝对路径 | 必须是 `plugins_dir` 之下的文件 |
| `mtime` | timestamp | 取得声明时该文件的修改时间 | 与 `size` 一起构成失效键 |
| `size` | u64 | 取得声明时该文件的字节数 | 同上 |
| `info` | PluginInfo | 类型、名称、描述、声明参数、声明的权限（net/read/write）、`login_from`、`url_pattern` 等 | 只读；不承载任何执行状态或进程句柄 |
| `options` | Option<PluginOptions> | 该插件 `pluginOptions()` 的声明（未导出则为 `None`） | 与 `info` 同一次失效周期 |
| `validated_at` | timestamp | 取得时刻 | 超过保守上限（默认 60s）即视为失效 |

**关系**：一个 `namespace` 至多一条条目；条目与 `PluginPool` 的常驻执行 worker（`workers` map）**彼此独立**——失效只丢弃声明条目，常驻 worker 是否重启仍由它自己的 mtime 判定。

**状态与转移**

```
        ┌──────────── 命中（mtime+size 未变且在 TTL 内）───────────┐
        │                                                          │
   ┌────▼─────┐   mtime/size 变化 / TTL 过期 / 插件上传替换删除  ┌──┴──────┐
   │ 已缓存    │ ─────────────────────────────────────────────▶ │ 需重新获取 │
   └──────────┘                                                └────┬─────┘
        ▲                                                          │ 一次真实加载（最小授权）
        └──────────────────── 成功（写入 mtime+size+info）──────────┘
```

**校验规则**

- V1：任何写入都以“现场读到的 `(mtime, size)`”作为键；不接受调用方传来的时间戳。
- V2：并发请求同一 `namespace` 时，只允许一次真实加载；其余等待其结果（不能退化成并行多次加载）。
- V3：加载失败不写缓存（错误照原样返回），下一次调用重试；不得缓存失败、也不得把旧条目当成成功结果。
- V4：缓存内容不得包含任何可用于绕过权限判断的信息（例如“已经允许访问的域名列表”之外的授权状态）；权限仍由每次执行的进程参数决定。
- V5：插件目录被删除/不可读时，发现结果为空，所有条目失效，入库流程不得因此失败。

---

## E2. 下载任务阶段计时（JobPhase，进程内，`JobStatus` 的追加字段）

| 字段 | 类型 | 说明 |
|---|---|---|
| `name` | enum | 六个固定阶段之一，见下表 |
| `started_at` | i64（epoch ms） | 与 `JobStatus.started_at` 同单位 |
| `ended_at` | Option<i64> | 结束时刻；任务失败或仍在进行时为 `None` |

**阶段词表（固定，顺序单调）**

| 阶段 | 覆盖区间 | 出现在 |
|---|---|---|
| `queued` | job 建立 → 拿到插件级 job 槽 | 每次下载 |
| `negotiate` | 插件 `execDownload` 执行期间 | 每次下载 |
| `waiting` | 等待传输许可（按域并发上限） | 每次下载 |
| `transfer` | 字节传输（含多资源逐个传输） | 每次下载 |
| `ingest` | 打包 / 拷贝 / 落库（到档案记录写成为止） | 每次下载 |
| `publish` | 元数据插件链 + 发布（`hidden` → `published`） | 每次下载 |

**校验规则**

- V6：`phases` 必须按上表顺序出现；后一阶段的 `started_at` ≥ 前一阶段的 `ended_at`（重试同一队列项时从 `queued` 重新开始，不追加第二轮）。
- V7：任一时刻至多一个阶段 `ended_at == None`；任务进入终态时所有阶段都必须有 `ended_at`（未走到某阶段则不出现该阶段条目）。
- V8：`phases` 为空时序列化时整体省略（旧客户端/旧服务互操作）；任务总耗时仍由 `started_at`/`finished_at` 表达。
- V9：不持久化。服务重启后 job 消失、`phases` 随之消失；队列行必须能在此情况下退回“仅总耗时”的展示。

**派生（前端展示用，不落库）**

- 排队 = `queued` + `negotiate` + `waiting`
- 下载 = `transfer`
- 入库 = `ingest` + `publish`
- 未归属间隙 = 总耗时 − 三段之和（不得计入“下载”）

---

## E3. 队列项上的元数据产出者（`DownloadQueueItem.metadata_preview_namespace`）

| 字段 | 类型 | 说明 |
|---|---|---|
| `metadata_preview_namespace` | Option<string> | 产出当前 `metadata_preview` 的插件命名空间（如 `metadata/ehentai`） |

**校验规则**

- V10：只在 `ensure_metadata_cached` **成功**拿到插件结果时写入；失败/跳过时不写（保持 `None`）。
- V11：与 `metadata_preview` / `metadata_preview_at` 一起失效：preview 被清空（重试时 `plugins.rs:1290-1295` 清 title/preview）则本字段一并清空。
- V12：`None` 是“入库阶段必须跑完整插件链”的唯一判据；不得用“preview 存在”代替（用户手动 Preview 也可能有 preview）。
- V13：序列化时为空则省略；旧客户端忽略即可（无行为依赖）。

---

## 不新增的数据

- 不新增 Redis 键、不新增磁盘文件、不新增数据库/索引结构。
- 不改变档案记录（`Archive`）的任何字段；标题/标签的落库仍走既有的 `update_then` + `reindex_title_and_tags`。
- 不改变插件协议（`pluginInfo`/`pluginOptions`/`execMetadata`/`execDownload` 的入参与语义）。
