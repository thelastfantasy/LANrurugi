# Feature Specification: 下载 → 入库的耗时构成与插件发现开销

**Feature Branch**: `feat/028-deletion-safety-net`（沿用当前分支，与 020–028 一致）

**Created**: 2026-11-24

**Status**: Draft

**Input**: 用户在 NAS beta 容器上实测后连续追问：「档案下载后的入库速度还是差强人意」→「主要是这里下载和档案目录在不同硬盘，temp 在 ssd 但漫画目录（入库最终目的地）在 hdd」→「不对，我看到的就是下载界面 ui 中"入库"的速度耗时很长」（附队列行截图：`入库中…`、耗时 17:30、平均 3.3 MB/s、873.6 MB）→「deno 不能多线程吗？还有，入库为什么也要交给 deno？不应该是 rust 的工作吗？」→「现在在队列中等待的时间似乎也算在下载时间里面了？」。要求把这一轮查到的全部问题整理进 spec，本轮只到 `tasks` + `analyze`，不进入实现。

## Context

### 实测基线（2026-11-24，NAS `lanrurugi-beta`，release `lanrurugi@1.0.0-beta.8`，7502 档案，12 核）

| 观测项 | 数值 | 来源 |
|---|---|---|
| `/temp`（下载暂存）写入 | 280 MB/s | 容器内 `dd` + `fsync` |
| `/temp` → `content`（跨文件系统拷贝） | **315 MB/s**（916 MB ≈ 3 s） | 容器内 `dd` + `fsync` |
| 生产环境真实入库拷贝（监控 `.ingest-*`） | 576 MB/2.1 s、341 MB/1.1 s、314 MB/1.1 s | 500 ms 采样 |
| 传输结束 → 拷贝开始 | 0.3 s | 同上 |
| `hidden → published`（单档，48 h，n=138） | p50 **1.88 s** / p90 **23.5 s** / p99 36.4 s / max **44.0 s** | 容器日志 |
| 其中 >20 s 的条目 | **19 / 138**，全部集中在一次 62 条批量的头两分钟 | 同上 |
| 撞 30 s 入库超时 | 2 条（`8adaff18` 44 s、`64253ab1` 36 s） | `WARN ingestion timed out` |
| 62 条批量整体 | 588 s / 11.5 GB / 聚合 19.5 MB/s | Redis 队列项 |
| `execDownload`（archiver.php 协商，48 h，n=129） | p50 **40.6 s** / p90 **106.5 s** / max 143.7 s | 容器日志 |
| `*.hath.network` 并发许可等待（48 h，n=67） | p50 **153 s** / p90 288 s / max **498 s** | 同上 |
| 单次 `deno run … plugin_info` 冷启动（空闲） | **18–19 ms** | 容器内直接调用 dispatcher |
| 一次入库触发的插件发现进程数 | **34**（= `plugins/**/*.ts` 总数） | `plugins/` 目录 |
| 满载时单档发现阶段耗时 | **19 s**（`hidden` 01:13:57.635 → 第一个插件 01:14:16.5） | 容器日志 |
| 一个 44 s 入库窗口内的插件进程启动 | **696 次** | `plugin network: host relay` 计数 |
| 截图那一行（队列项 `9ba8f386…`，916 MB） | 总 17:36 = 排队+协商 ≈13:03 + 传输 4:27 + 入库 ≈6 s | 队列项 + 日志 |

结论：跨盘拷贝不是瓶颈（315 MB/s）；**空闲时入库 ≈2 s、批量并发时 20–44 s**；用户看到的 17:30 是整条任务的墙钟，其中 13 分钟在排队与 e-hentai 协商。

### 问题清单（Investigation Record）

| # | 问题 | 证据（`@ed7d231`） | 后果 |
|---|---|---|---|
| G1 | **插件声明信息每次调用都新起一个 Deno 进程** | `crates/lanrurugi-plugin/src/pool.rs:388-410` `plugin_info` 每次 `Worker::spawn`；同文件 `:122-196` 的 spawn 参数；对比 `execute`（`:569-601`）复用常驻 worker | 一次入库 34 次 V8 进程启动；8 条并发 = 272 进程抢 CPU |
| G2 | **每次入库都为“发现插件”付这 34 次开销** | `crates/lanrurugi-api/src/plugins.rs:1617-1636`（`discover_namespaces` + 逐 namespace `plugin_info` 筛 kind）；`download_manager/ingest.rs:755` 在 publish 前调用 | `hidden → published` p50 1.88 s 里约 0.65 s 纯属发现；满载时放大到 19 s |
| G3 | **同一份发现开销还有两处** | `plugins.rs:3675-3685`（`ensure_metadata_cached`，下载开始时抓标题）；`plugin_options`（`pool.rs:417-440`）同样每次 spawn，`GET /api/plugins/{kind}` 实测 866 ms/次、p99 5.2 s | 一次下载 = 68 次进程启动；设置/上传页每次加载也全量 spawn |
| G4 | **下载入库后把同一套元数据插件再跑一遍** | `download_manager/ingest.rs:751-758`；开始下载时 `ensure_metadata_cached` 已抓到 title/tags（`plugins.rs:2949`） | 重复的 chaika 文本搜索 + E-H API 请求；这正是 G2 的触发点，也是 025 的 hidden 窗口被拉长的原因 |
| G5 | **队列行的“耗时”把排队算进了下载，速率却只算传输窗口** | `download_queue.rs:1357` `item.started_at = now_ms()`（在 `start_download` 之前、job 槽与 domain permit 都在其后）；`apps/frontend/src/pages/Upload/QueueItemRow.tsx:118-131` `durationMs = job.started_at ?? item.started_at → now`；`average_bytes_per_sec` 只覆盖有字节流动的窗口（`crates/lanrurugi-core/src/jobs.rs:60-77`） | 同一条显示“耗时 17:30 · 平均 3.3 MB/s · 873.6 MB”，端到端实际 0.87 MB/s；用户把 17:30 理解为入库耗时 |
| G6 | **UI 只有一个布尔把“入库”标出来，且没有自己的计时** | `QueueItemRow.tsx:125` `ingesting = state === "downloading" && transferring === false`；`:491-526` 用它替换读数 | 无法回答“入库花了多久”，只能靠整行 `耗时` |
| G7 | **下载/入库链路在追踪系统里不可见** | 下载 job 与入库都是游离 `tokio::spawn`（`plugins.rs:2360+`）；rustrak（project 1 = 生产）只有 `http.server` transaction，没有任何 download/ingest span | 每次排查只能翻容器日志；OSS 用户无从自查 |
| G8 | **watcher 的重复 ingest 撞 30 s 超时，污染失败统计** | `crates/lanrurugi-scanner/src/pipeline.rs:36` `INGEST_TIMEOUT = 30s`；`full_scan.rs:393` 把它计入扫描结果；触发场景是 `catalogue_staged_file` 持有 `content_lock` 时 watcher 对同一文件再 ingest 一次 | 2 条 `WARN ingestion timed out` 实为无害的重复 ingest，却被当作失败 |
| G9 | **插件声明的 namespace 与安装路径不一致** | `plugins/metadata/chaika.ts:26` `namespace: "trabant"`（模板残留）；实测 `plugin_info` 返回 `{"namespace":"trabant","type":"metadata",…}` | 按 `info.namespace` 的展示/匹配与路径推导的 `metadata/chaika` 不一致 |

### 与宪法的一致性

- **Principle IV** 明确写：“To bound latency, plugin execution SHOULD use a persistent worker pool with dynamic `import()` dispatch rather than spawning a fresh Deno process per invocation.”——G1/G3 是这条 SHOULD 的现存违例（`plugin_info`/`plugin_options` 走了 per-invocation spawn）。
- **Principle III**：“Component/engine choices MUST weigh runtime memory and startup footprint as a first-class criterion”——G1/G3 恰好是在最热的路径上按次数付进程启动成本；同条对“循环里逐项 spawn”的批评同样适用于“循环里逐插件 spawn”。
- **Principle II**：本特性的任何新增字段（阶段计时）只能追加、必须可选，不得改变既有响应形状。
- **Principle VII**：G5/G6 是纯前端展示变更，最终登签由用户在自己的浏览器里完成。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 批量下载时“入库中…”不再被插件发现拖成几十秒 (Priority: P1)

用户一次点“Start All”下载几十本（实测 62 条）。每条下载完成后，队列行进入“入库中…”直到档案可搜索。当前这一阶段在空闲时约 2 s，但在 8 条并发入库时会劣化到 20–44 s，其中绝大部分时间不是在写书库，而是在为“这个站有哪些插件、它们是什么类型”反复启动 34 个一次性运行时进程。

**Why this priority**: 这是用户直接抱怨的现象（截图里那一行 `入库中…` 长时间不动），也是唯一能在入库链路上真正砍掉的成本；修好它，入库回到“拷贝耗时 + 一次元数据调用”的量级。

**Independent Test**: 在测试部署上一次排入 ≥8 条下载，全部完成后记录每条从“传输结束”到“可被列表/搜索命中”的耗时；与改动前的 20–44 s 对比。

**Acceptance Scenarios**:

1. **Given** 8 条下载几乎同时完成传输，**When** 它们依次进入入库，**Then** 每条的“入库中…”阶段 ≤ 5 s，且没有任何一条撞上每档入库超时。
2. **Given** 插件目录自上次询问后没有任何文件变化，**When** 连续入库 20 条档案，**Then** 系统为“发现插件”启动的运行时进程数为 0。
3. **Given** 用户修改（或上传、删除）了 `plugins/` 下的一个 `.ts`，**When** 下一次入库或下一次插件列表请求发生，**Then** 新插件立即出现在结果里、被删除的立即消失，不需要重启服务。

---

### User Story 2 - 队列行把“排队 / 下载 / 入库”三段分开显示 (Priority: P1)

用户看队列行时，`耗时` 从点“开始”那一刻起就在计，包含等待插件并发槽、archiver 协商、等待下载许可、传输、入库；而旁边的 `平均 X MB/s` 只算真正有字节流动的窗口。两个数字口径不同，再加上 `入库中…` 与总耗时并排，读起来就像“入库花了 17 分钟”。

**Why this priority**: 不改的话，即便 US1 把入库修到 3 s，用户仍会把排队时间误记为入库时间，问题会反复被报告。

**Independent Test**: 取一条包含协商、排队、传输、入库的下载，比对行内显示的三段计时与后端记录的阶段耗时。

**Acceptance Scenarios**:

1. **Given** 一条下载在 job 槽前等了 6 分钟才拿到传输许可，**When** 它处于“下载中”，**Then** 行内显示的“下载时长”只统计真正传输的时间，排队时间单独显示（例如“排队 6:00”）。
2. **Given** 一条下载已完成传输、正在入库，**When** 用户看这一行，**Then** 能看到“入库”阶段自己的耗时（秒级），并明确知道它不是整条任务的总耗时。
3. **Given** 一条下载已结束，**When** 用户展开/悬停这一行，**Then** 能看到三个阶段的最终耗时与任务总耗时。
4. **Given** 服务在下载过程中重启（job 已丢失），**When** 用户仍看到该队列行，**Then** 行内不得显示伪造的阶段耗时，须退回“仅总耗时”的展示。

---

### User Story 3 - 下载入库不再重复跑一遍元数据插件 (Priority: P2)

e-hentai 下载插件在“开始下载”时就会抓一次元数据（标题/标签），用于队列行的标题预览。当前入库阶段又会把同一条 chaika + E-H 链完整跑一遍——网络请求一样、结果一样，只为了让档案从 `hidden` 变 `published`。

**Why this priority**: 它既是 US1 的触发点，也是纯浪费的外部请求（会让站点限流），但单独修不掉 US1 的进程开销，所以排在 P2。

**Independent Test**: 用一条 e-hentai 单资源下载，统计从“点开始”到“归档可搜索”期间发往 chaika/E-H 的元数据请求次数。

**Acceptance Scenarios**:

1. **Given** 开始下载时元数据抓取成功并已写入队列项，**When** 该档案入库，**Then** 不再对同一条来源发起第二遍元数据请求，且档案仍带着正确的标题/标签被发布。
2. **Given** 开始下载时元数据抓取失败（插件报错、站点不可达、或用户关掉了自动抓取），**When** 该档案入库，**Then** 仍然执行完整的元数据链（不能因为“曾经失败过”就跳过）。
3. **Given** 用户手动点“Fetch Metadata”，**When** 请求发出，**Then** 仍然真实调用插件（不得被任何缓存挡住）。

---

### User Story 4 - 从追踪系统就能看出下载/入库各阶段耗时 (Priority: P2)

维护者（以及接入自查的用户）打开追踪服务时，只能看到 HTTP 请求；一次下载任务从协商到入库的六段耗时完全不可见，因此每次“为什么这么慢”都要登机器翻容器日志。

**Why this priority**: 它不是用户直接感知的延迟，但决定了同类问题下次是否还要花一轮排查；在 US1/US2 落地后价值更高（能看到收益）。

**Independent Test**: 触发一次下载，在追踪服务里找到对应 trace，核对各阶段 span 与时长。

**Acceptance Scenarios**:

1. **Given** 一次下载任务开始，**When** 它在追踪服务中结束，**Then** 该 trace 下能看到协商、排队、传输、拷贝、落库、元数据六个阶段各自一个子 span，且父 span 时长等于各子段之和（含未归属的间隙）。
2. **Given** 追踪功能被关闭（未配置 DSN），**When** 下载与入库正常进行，**Then** 行为与耗时完全不受影响（埋点不得成为失败路径）。

---

### User Story 5 - 诊断噪音与声明一致性 (Priority: P3)

扫描/诊断页把“watcher 在等同一个文件的入库锁、等满 30 s 后退让”记成入库超时失败；插件列表里 `metadata/chaika` 声明的 namespace 是 `trabant`。

**Why this priority**: 不影响用户可见延迟，但会误导下一次排查（本次调查就被这两条误导过一次）。

**Independent Test**: 复现“下载入库 + watcher 同文件”场景，检查扫描结果与日志级别；检查插件列表/详情里 chaika 的标识。

**Acceptance Scenarios**:

1. **Given** 下载路径正持有某个内容 id 的入库锁，**When** watcher 对同一文件发起重复 ingest 并等锁超时，**Then** 这条记录不计入扫描失败数，日志不出现与真正超时同级的告警。
2. **Given** 一个插件文件声明了与自身安装路径不一致的 namespace，**When** 它被加载或被列出，**Then** 用户可以看出来（诊断/列表里有明确提示），而不是静默地显示成另一个名字。

---

### Edge Cases

- **插件文件在缓存有效期内被替换，而修改时间与大小恰好不变**（`cp -p`、某些网络文件系统）：缓存必须有一个保守的上限（或额外的内容指纹）避免一直用旧声明。
- **两个入库任务同时要求发现同一批插件**：只能是“一次发现、多份复用”，不能退化成“两份并行发现”。
- **自定义插件目录（`custom/`、wizard 的草稿目录）**：缓存必须覆盖上传/丢弃草稿这两条路径，丢弃草稿后不得留下条目。
- **插件目录被清空或不可读**：发现结果为空，入库照常完成（不得让档案卡在 `hidden`）。
- **元数据抓取成功但用户随后改了标题/标签**：入库阶段不做第二轮插件，也不能把用户的手改覆盖掉。
- **多资源打包（如 Pixiv 每页一张图）**：阶段计时里“拷贝”包含打包；`入库中…` 的时长必须仍与实际相符。
- **服务重启**：进行中的下载任务丢失，队列行的阶段计时随之消失；不得显示上一轮残留的阶段数值。
- **e-hentai 这类“协商比传输还长”的来源**：排队与协商必须分开显示，否则“下载 13 分钟”仍会被误读为传输慢。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 系统 MUST 复用已经取得过的插件声明信息（类型、参数、声明的权限、来源路径），在插件文件未变化时不得为每次入库/每次列表请求重新询问插件运行时。
- **FR-002**: 插件声明缓存 MUST 随插件文件变化（修改时间、大小）失效，且插件上传、替换、删除后对下一次调用立即生效，无需重启服务。
- **FR-003**: 插件声明缓存 MUST NOT 改变插件的隔离与权限边界——每个插件仍以自己的权限运行，缓存只保存“它声明了什么”，不保存可复用的执行环境。
- **FR-004**: 稳态（插件文件未变化）下，一次入库或一次插件列表请求为“发现插件”启动的运行时进程数 MUST 为 0，与已装插件数量无关；冷启动允许每个插件各启动一次，但 MUST NOT 在后续调用中重复启动（research.md R1 已否决“一个进程加载全部插件”的 manifest 方案：`--allow-net`/`--allow-read` 是进程级参数，合体会把权限变成并集，违反 Principle IV）。
- **FR-005**: 下载入库时，系统 MUST NOT 再次执行“本次下载的启动阶段已经成功执行过的那一个元数据插件”；其余已启用插件 MUST 照常执行（legacy 对每条新档案跑启用插件的语义不变）。若启动阶段未成功（关闭自动抓取 / 抓取失败 / 无匹配插件），入库阶段 MUST 执行完整链条。
- **FR-005a**: 跳过某个插件时，系统 MUST 仍然把该插件在启动阶段产出的标题与标签应用到新档案上（今天这一步由被跳过的插件在入库阶段顺带完成，不得因此丢失）。
- **FR-006**: 用户手动触发的“Fetch Metadata” MUST 始终真实调用插件，不受任何缓存影响。
- **FR-007**: 每个下载任务 MUST 记录分阶段耗时，阶段名固定为 `queued`（排队等候）、`negotiate`（来源协商）、`waiting`（等候传输许可）、`transfer`（传输）、`ingest`（打包/拷贝/落库/索引）、`publish`（元数据插件 + 变为可见）；记录 MUST 在任务结束后对 API 调用方可见。
- **FR-008**: 队列行 MUST 分开显示排队时长、下载（传输）时长、入库时长；三者之和与任务总耗时的差额（未归属间隙）不得被计入“下载时长”。
- **FR-009**: 队列行 MUST 明确标注“总耗时含排队”这一类说明，或等价地把总耗时与三段耗时区分开，避免用户把排队计为下载或入库。
- **FR-010**: 下载与入库链路 MUST 在追踪系统中留下分阶段 span（父 span 覆盖整个任务），且埋点关闭时不得影响任务行为与耗时。
- **FR-011**: watcher 因等待同一内容 id 的入库锁而超时 MUST 与真正的入库超时区分：前者不计入扫描失败数、不以同等告警级别呈现。
- **FR-012**: 插件声明的 namespace 与其安装路径不一致时，系统 MUST 在诊断/列表层暴露这一不一致（而不是静默采用其中一个）。
- **FR-013**: 本特性新增的任何 API 字段 MUST 是可选追加字段，既有客户端在缺少它们时行为不变（Principle II）。
- **FR-014**: 冷启动时若多个请求同时发现整个插件库，系统 MUST 把“临时声明进程”（只回答“这个插件声明了什么”然后退出）的并发数限制在由本机核数推导出的固定上限（2–8）内，该上限 MUST NOT 暴露为用户设置；已建立并复用的常驻插件 worker MUST NOT 为腾出名额而被终止（那会杀掉可能正在被调用的进程）。

### Key Entities

- **插件声明条目（Plugin Descriptor）**：一个插件对其身份与需求的声明——安装路径推导出的命名空间、类型（metadata/download/login/script）、名称与描述、声明参数、声明的权限（网络域名、读写）、来源文件的修改时间与大小。它是可缓存的只读事实，不含任何执行状态。
- **下载任务阶段计时（Job Phase Timing）**：一次下载任务的分段墙钟——排队、协商、等候传输许可、传输、入库、元数据，以及任务总耗时；随任务生命周期存在，任务结束后随任务记录一起可读。
- **队列行展示（Queue Row Reading）**：前端由上述阶段计时派生出的三段读数与总耗时，以及“当前处于哪一段”的判定（排队 / 传输 / 入库）。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 空闲时单条入库（自传输结束到档案可被搜索命中）p50 ≤ 1.0 s（当前 1.88 s）。
- **SC-002**: 8 条下载同时进入入库时，单条入库 p95 ≤ 5 s（当前 23.5 s），且不再出现任何一条 >20 s（当前 19/138）。
- **SC-003**: 稳态下一次入库为“发现插件”启动的运行时进程数从 34 降到 0（服务启动后第一次入库允许仍为 34，之后不再重复）；一次 62 条批量下载期间，插件发现相关进程启动总数从约 2100 降到 0（仅在插件文件变化后重新发现）。
- **SC-004**: 队列行显示的三段耗时与后端记录的阶段耗时偏差 ≤ 1 s；“下载时长”内不含排队时间（用一条排队 ≥5 分钟的样本验证）。
- **SC-005**: 一次 e-hentai 单资源下载，全过程对**同一个**元数据插件的调用次数 ≤ 1（当前 2 次：开始抓取 + 入库重跑），且入库后档案的标题/标签与开始阶段抓到的结果一致。
- **SC-006**: 追踪系统里一次下载任务可见 ≥6 个阶段 span，且父 span 覆盖到任务结束。
- **SC-007**: 复现“下载入库 + watcher 同文件”场景时，扫描结果中的失败数为 0，且不再出现与真正超时同级的告警。

## Assumptions

- 阶段计时只存在于任务生命周期内，不新增持久化结构（与 002 job console 的既有取舍一致：服务重启后丢弃）。
- 保留 025 的 “hidden until its metadata run finishes” 发布语义；本特性只缩短该窗口，不改成“先发布、后台补元数据”（那是另一个产品决策）。
- 插件发现缓存是进程内缓存，不新增 Redis/磁盘结构；多实例部署时每个实例各自发现一次（当前部署是单实例）。
- 插件 ABI 不变：`pluginInfo()` / `pluginOptions()` / `execMetadata` / `execDownload` 的签名与语义保持兼容，legacy 插件无需改动。
- 排队/协商时长本身不做优化（e-hentai 的 archiver 与 H@H 传输速度不由本项目控制）；`*.hath.network` 的并发上限属于用户可改的运行时配置，不在本特性范围内调整。
- 追踪 span 的落地方式沿用现有 `diagnostics_settings` / rustrak 通道，不新增第三方依赖。
- 进程上限按机器自动推导（`lanrurugi_plugin::pool::worker_limit()` = 核数/2，夹在 2–8），不提供设置项：常驻 worker 已经“每个插件一个、被该插件的所有并发调用共用”，真正会失控的是冷启动那一批临时声明进程；用淘汰常驻 worker 的方式限数会杀掉可能正在被调用的进程，属于用风险换数量，故不做。
