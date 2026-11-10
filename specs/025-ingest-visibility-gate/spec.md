# Feature Specification: 入库可见性门禁

**Feature Branch**: `feat/share-bind-interface`（沿用当前分支；speckit 无 before 钩子，不新建分支）

**Created**: 2026-11-10

**Status**: Draft

**Input**: 用户报告（m00001）：beta5 里「还没有完全入库的内容会提前出现在搜索结果里（阅读页 URL 也会提前有效化）」，要求「完全入库（入库和 meta 插件自动执行完成）再出现在搜索结果里并且阅读页有效化」。用户要求先调查、不直接改代码，并把当前查出的问题记录进 spec。

## Context

一次下载进入书库要经过四段：字节落到临时区 → 写入档案记录并建搜索索引/缩略图（下称 cataloguing）→ 依次执行所有已启用的 metadata 插件 → 对用户可见。用户要求把「可见」这件事严格放到最后一段结束之后：

- 搜索（含 Tankoubon 分组、访客分类限制、`suggest` / `random` / OPDS）不得命中；
- 阅读页及其数据接口（metadata / files / page）不得可用；
- 最后一个 metadata 插件返回后，两者同时生效，且带插件写入的标签。

beta5（tag `v1.0.0-beta.5`）的实际顺序是「先写记录（`file` 为空）→ 再复制字节 → 修正 `file` → 跑插件」，所以整个复制期间记录就已存在：搜索能命中一张没有标签的卡，metadata 返回 200（只有 files/page 因空路径返回 409）。

beta5 之后有一个提交专门处理这件事：`b1723b6`（2026-11-08，`fix(ingest): 入库未完成的档案不再可见…`，未发布）。它引入 `LANRURUGI_PENDING_VISIBILITY:<id>` 标记（`crates/lanrurugi-storage/src/keys.rs`），在 cataloguing 之后写入（`crates/lanrurugi-api/src/download_manager/ingest.rs:458`）、插件跑完后删除（同文件 `:528`），并在搜索候选集里过滤（`crates/lanrurugi-search/src/engine.rs:471-473`）。**但这条修复在当前代码上没有达到目的**：标记与读取分属不同 Redis 逻辑库，访客分组搜索会在过滤之后把档案重新加回，阅读页三个接口根本不查标记，标记写入时机也晚于搜索索引。逐条证据见下一节。

## 现状与缺陷证据（Investigation Record）

调查对象：当前 HEAD 源码（`feat/share-bind-interface`，`5784187`）+ 本机 dev 容器里由 `657ebf4` 构建的二进制（`/api/info` 报 `1.0.0-beta.5`），以及 Redis 里的真实档案 `fd3043a88275bfbfcd8ae091c8cc4b304166448e`。验证方式：直接往 Redis 写标记，再用未登录（`guest_visitor`）与带 `groupby_tanks=false` 的请求对照。

| # | 缺陷 | 证据 |
|---|------|------|
| D1 | **标记写错逻辑库**：写入用 `state.redis.config`（DB 2），搜索读取用 archive 连接（DB 0）。两个逻辑库 key 空间隔离，所以浏览器搜索永远看不到标记。 | `crates/lanrurugi-storage/src/redis.rs`：`DB_ARCHIVE=0`、`DB_CONFIG=2`、`DB_SEARCH=3`；`ingest.rs:245-263` 的 `mark_pending_visibility` / `clear_pending_visibility` 用 `state.redis.config.get()`；`engine.rs:188` 的 `pending_visibility` 接收 `archive_conn`。dev 实测：只写 DB 2 时搜索照常命中；`MONITOR` 里应用执行的是 `[0 ...] KEYS LANRURUGI_PENDING_VISIBILITY:*`。 |
| D2 | **访客分组搜索把 pending 档案加回来**：候选集先 `retain` 掉 pending（`engine.rs:473`），随后 `restrict_to_archive_ids` 的 `filtered.extend(...)`（`:490-504`）又把访客可见的全部 id 并入，`retain(allowed_set)` 只做交集、不排除 pending。 | `engine.rs:441` 在 `restrict_to_archive_ids.is_some()` 时 `member_aware=false`；`crates/lanrurugi-api/src/search.rs:510` 仅在 `AuthMethod::GuestVisitor` 时设置该字段；dev 的 `LRR_CONFIG guestmode=1`，未登录 `/api/search` 返回 200。实测：把标记放进 DB 0 后，`/api/search`（默认分组）仍返回该档案，`groupby_tanks=false` 与 `/api/search/archives` 返回 `[]`。 |
| D3 | **阅读页完全没有门禁**：metadata / files / page 只检查 `file` 是否为空；cataloguing 时 `file` 已是最终路径，所以插件运行期间三者也全部可用。 | `crates/lanrurugi-api/src/archives.rs:408`（`get_archive_metadata` → `resolve_search_entry`，仅 `file.trim().is_empty()`）、`:4470`（`get_files`，空 file 才 409）、`:3377`（`serve_page`，空 file 才 409）。实测（标记在 DB 0 期间）：metadata 200、files 200、`/page?path=_1.jpg&optimize=1` 200（360096 字节）。 |
| D4 | **标记晚于索引写入**：`ingest_file_with_policy` 内部先 `archives.save` → 写旁车 → `index_new_archive` → 生成缩略图再 save，返回后才 `mark_pending_visibility`。这段窗口里档案已经可搜。 | `crates/lanrurugi-scanner/src/pipeline.rs:491`（`catalogue_new_archive`）；`ingest.rs:458`。`archive_id` 在 cataloguing 之前已由 `size_aware_id` 算出，说明标记可以更早写。 |
| D5 | **枚举接口不过滤**：`GET /archives` 与 `GET /archives/untagged` 直接 `list_all()`，pending 档案会出现在管理员/会话的列表里（访客两端点 403，所以这是登录用户侧的泄漏）。 | `crates/lanrurugi-api/src/archives.rs:304`（`list_archives`）、`:331`（`untagged_archives`）。 |
| D6 | **标记会自己过期，且没有崩溃清理**：TTL 3600 秒，进程在插件中途崩溃/标记没删干净，1 小时后档案就带半套标签出现在搜索里；mark/clear 都是 best-effort，Redis 写失败就直接跳过（提前可见）；全仓没有任何测试覆盖这条路径。 | `keys.rs:39` `PENDING_VISIBILITY_TTL_SECS=3600`；`ingest.rs:245-263` 注释明说 best-effort；`git log -S PENDING_VISIBILITY` 只有 `b1723b6`；`grep` 全仓 8 处引用、0 个测试。 |
| D7 | **上传的二次插件 pass 发生在标记清除之后**：上传走 `catalogue_staged_file`（内部跑一遍插件并清标记），随后 `upload.rs:902` 再跑一遍插件以叠加用户标签，第二遍期间档案已经可见。（订阅不另跑 pass：订阅只把 URL 放进下载队列，走的是同一条下载入库路径；`subscriptions/runner.rs:902` 那次调用是已发布档案在源变更后的重抓，不属于本 feature 的新档案窗口。） | `crates/lanrurugi-api/src/upload.rs:867-910`；订阅入队见 `crates/lanrurugi-api/src/subscriptions/runner.rs:686-718`。 |
| D8 | **Tankoubon 卡片成员列表会带出 pending 成员**：`fold_tanks` 用「表达式命中的集合」算每个 tank 的命中成员（`engine.rs:706-739`），而 D2 的 extend 已经把 pending id 放进该集合；`search.rs:548-600` 再用这份 `member_matches` 取成员标题渲染到卡片上。 | `engine.rs:706-739`；`crates/lanrurugi-api/src/search.rs:548-600`。 |

补充事实（影响方案边界）：

- 真正写记录/建索引的入口是 `catalogue_staged_file`（下载、上传、共享导入、旧文件名冲突收编都走它）；scanner 的 watcher / full scan 直接走 `ingest_file_with_policy`，不跑 metadata 插件，也没有标记——这是既有设计，不在本 feature 范围内。
- meta 插件是串行 `await`，全部返回才算结束（`crates/lanrurugi-api/src/plugins.rs:1598`）；插件失败不会中断入库。
- 启动时已有统一的「收敛崩溃中间态」流程（`crates/lanrurugi-server/src/main.rs:745` 的 `repair_zombie_archives`，实现于 `main.rs:1363`），可作为 pending 孤儿清理的挂载点。

## User Scenarios & Testing

### User Story 1 - 下载未完成时搜索不到 (Priority: P1)

用户排了一个下载任务。字节下完、记录已写入、metadata 插件正在跑（可能几十秒）。此时用户在书库搜索标题、标签或作者，无论管理员还是访客、无论是否按 Tankoubon 分组、无论走网页搜索还是 OPDS/suggest/random，都不应该看到这张卡；Tankoubon 卡片的成员提示里也不应出现它。

**Why this priority**: 这是用户报告的核心现象——「还没有完全入库的内容提前出现在搜索结果里」。搜索是发现入口，泄漏一张无标签卡会误导用户点进一个还没准备好的阅读页。

**Independent Test**: 用一个可控延迟的测试 metadata 插件（或直接对真实档案写 pending 标记模拟）让「已 cataloguing、插件未结束」窗口持续 ≥30 秒，在此期间对上述每种搜索入口轮询断言 0 命中；窗口结束后同一查询必须命中。

**Acceptance Scenarios**:

1. **Given** 一个刚完成 cataloguing、metadata 插件尚未返回的下载档案，**When** 管理员用标题/标签/source 搜索（分组与未分组各一次），**Then** 结果中不出现该档案，且 `recordsFiltered` 不把它计入。
2. **Given** 同上窗口，**When** 未登录访客（`guest_visitor`，分类对其可见）执行同样的搜索，**Then** 结果同样不出现它（覆盖 D2 的 extend 路径）。
3. **Given** 同上窗口且该档案是一个 Tankoubon 的成员，**When** 搜索命中同 tank 的另一个成员，**Then** 出现的 Tankoubon 卡片成员列表/提示中不包含该档案。
4. **Given** 同上窗口，**When** 调用 `GET /api/search`, `GET /api/search/archives`, `GET /api/search/ids`, `GET /api/search/suggest`, `GET /api/search/random` 与 OPDS 搜索，**Then** 全部不返回该 id。
5. **Given** 插件在 30 秒后返回，**When** 立即重跑同一查询，**Then** 该档案出现（无需等待索引重建或下一轮扫描）。

---

### User Story 2 - 下载未完成时阅读页 URL 无效 (Priority: P1)

同一个窗口内，即使用户从历史记录、书签或直接粘贴 URL 打开 `/reader?id=<id>`（或直接请求其数据接口），阅读页也不应加载出内容，而应表现为「不存在/尚未就绪」；插件结束后同一个 URL 立即可用。

**Why this priority**: 与 User Story 1 是同一个承诺的两半；只挡搜索不挡 URL，等于把「未完成」状态暴露给任何拿到 id 的人（例如下载队列页面、日志、共享链接）。

**Independent Test**: 窗口内请求 metadata/files/page 三个端点，断言返回 404（或等价的不可用状态）且不返回任何页字节；窗口结束后断言三者都成功，且 metadata 里带上插件写入的标签。

**Acceptance Scenarios**:

1. **Given** 插件未结束，**When** 请求 `GET /api/archives/{id}/metadata`，**Then** 返回 404，响应体不含标题、标签、页数。
2. **Given** 插件未结束，**When** 请求 `GET /api/archives/{id}/files` 或 `GET /api/archives/{id}/page?...`，**Then** 返回 404，不返回任何页字节。
3. **Given** 插件未结束，**When** 打开 `/reader?id={id}`，**Then** 页面进入错误/空状态，不显示任何页。
4. **Given** 插件在 30 秒后返回，**When** 立即请求上述三者，**Then** 全部成功，metadata 的标签包含插件输出。

---

### User Story 3 - 失败与崩溃不会留下永久隐藏的档案 (Priority: P2)

插件报错、插件超时、进程在插件中途被杀、Redis 里残留了上一次的标记——这些情况下档案最终都必须变得可见、可读，不能永远消失；同时，任何在本次 ingest 之前就已经发布的档案都不能因为重复下载/重复入库而被重新隐藏。

**Why this priority**: 门禁的失败模式必须是「晚一点可见」，不能是「永远不可见」。用户能接受插件失败后带半套标签出现，不能接受书库里少一本书。

**Independent Test**: 分别模拟插件返回错误、进程重启（保留 pending 标记）、重复下载已存在档案，断言前两者在有界时间内可见，后者从始至终可见。

**Acceptance Scenarios**:

1. **Given** metadata 插件返回错误，**When** 该次 ingest 结束，**Then** 档案被发布（带已收集到的标签），搜索与阅读页均可用。
2. **Given** 服务在插件执行中途被杀死，Redis 里留下 pending 标记，**When** 服务重新启动，**Then** 启动收敛流程在 60 秒内清除该标记，档案可见可读（不会永久隐藏）。
3. **Given** 一个已发布档案被再次下载（内容相同，`Unchanged` / 重复拒绝），**When** 处理结束，**Then** 它在整个过程中始终可见、可读。
4. **Given** 一个已发布档案被替换内容后重新入库（rekey），**When** 新记录进入插件阶段，**Then** 新 id 在插件结束前不可见；旧 id 的既有可见性不受影响。

---

### User Story 4 - 上传/共享导入/订阅路径遵守同一承诺 (Priority: P3)

用户通过上传、共享导入或订阅自动下载得到的档案，在「记录已写、插件还在跑」的窗口内同样不应出现在搜索里或可读；包括上传在入库后为叠加用户标签而执行的第二次插件 pass。

**Why this priority**: 这些路径与下载共用入库代码（`catalogue_staged_file`），且已经会触发 metadata 插件；如果只修下载，同一类泄漏会在上传/共享导入上继续存在。订阅自动下载只是把 URL 放进下载队列，走的就是同一条下载路径，因此由同一个门禁覆盖。但它们不是用户本次报告的场景，所以排在 P1/P2 之后。

**Independent Test**: 对上传与共享导入各跑一次带慢插件的入库，断言窗口内搜索/阅读不可用、结束后可用。

**Acceptance Scenarios**:

1. **Given** 上传的档案已 cataloguing、第二遍插件尚未结束，**When** 搜索或请求阅读页，**Then** 不可见/不可读。
2. **Given** 共享导入的档案仍在插件阶段，**When** 搜索或请求阅读页，**Then** 不可见/不可读。
3. **Given** 上述任一路径的插件全部结束，**When** 立即查询，**Then** 可见且 metadata 含插件标签。

### Edge Cases

- **没有启用任何 metadata 插件**：cataloguing 结束即发布，不得多等一轮。
- **插件执行期间服务重启**：见 User Story 3 场景 2；恢复后要么补跑插件、要么直接发布，两者都可接受，但必须有界。
- **Redis 在写标记时不可用**：入库必须失败（记录本身也在 Redis 里，写不进去就没有档案），而不是「跳过标记、让档案提前可见」。
- **Redis 在清标记时不可用**：档案短暂停留在隐藏态；必须在有限时间内由重试或启动清理发布，不能永久隐藏。
- **同一档案并发两次插件 pass（上传）**：只有最后一次 pass 结束才发布；中间那次结束不能提前放行。
- **pending 档案属于一个已发布的 Tankoubon**：tank 卡片不得显示它，也不得因它而让 tank 出现在只应命中其它成员的结果里。
- **访客可见分类与 pending 相交**：分类可见不等于档案可见；两者都满足才可见（D2）。
- **直接猜测/枚举 id**：阅读页接口统一返回 404，不返回「正在入库」之类的差异化信息，避免确认 id 存在。
- **旧版本遗留的 pending 标记**（beta5 之后到本 feature 之前写入 DB 2 的键）：启动清理不得把它们误当成新档案的门禁；它们本来就从未生效。

## Requirements

### Functional Requirements

- **FR-001**: 系统 MUST 把「已 cataloguing、尚未完成本次 ingest 全部 metadata 插件」的档案定义为未发布（pending），并在该状态下对所有发现入口隐藏它。
- **FR-002**: 搜索类入口 MUST 全部过滤 pending 档案：`/api/search`（分组与未分组）、`/api/search/archives`、`/api/search/ids`、`/api/search/suggest`、`/api/search/random`、OPDS 搜索，以及任何内部复用同一搜索管线的调用（含统计/推荐读取搜索结果的地方）。
- **FR-003**: 过滤 MUST 在候选集的所有后续变更之后仍然成立——尤其是访客的 `restrict_to_archive_ids` 收窄之后；任何「先过滤、后并入可见 id 集合」的顺序都视为不满足。
- **FR-004**: Tankoubon 聚合结果 MUST 不暴露 pending 成员：pending 成员不得成为 tank 被折叠进来的理由，不得出现在卡片的成员列表/提示中；一个只有 pending 成员的 tank 不得出现在结果里。
- **FR-005**: 阅读页数据接口 MUST 在 pending 期间不可用：`GET /api/archives/{id}/metadata`、`GET /api/archives/{id}/files`、`GET /api/archives/{id}/page` 返回 404，且响应体不包含标题、标签、页数或页字节。
- **FR-006**: 发布 MUST 发生在「本次 ingest 触发的最后一个 metadata 插件返回之后」；插件成功、失败、无插件可跑都算完成，不得因插件错误而阻塞发布。
- **FR-007**: 发布 MUST 是即时的：发布后第一次搜索/阅读请求就能命中，不依赖索引重建、定时任务或缓存过期。
- **FR-008**: 门禁 MUST NOT 依赖会在完成前自然到期的计时器；隐藏状态只能由显式的「发布」动作或启动收敛清除。
- **FR-009**: 门禁 MUST 覆盖所有会跑 metadata 插件的入库路径：下载、上传（含入库后的第二次插件 pass）、共享导入、订阅自动下载。
- **FR-010**: 已发布档案 MUST NOT 因为重复下载、重复入库、插件补跑或任何后台任务而被重新隐藏。
- **FR-011**: 写入 pending 状态失败 MUST 使该次入库失败，而不是让档案提前可见；清除失败 MUST 被重试并在启动收敛中兜底。
- **FR-012**: 服务启动时 MUST 收敛崩溃遗留的 pending 状态：在 60 秒内让受影响档案变为可见可读（或补跑插件后再发布），且不得永久隐藏任何档案。
- **FR-013**: 管理员侧枚举接口（`GET /api/archives`、`GET /api/archives/untagged`）MUST 同样不返回 pending 档案。
- **FR-014**: 系统 MUST 记录 pending 的写入、发布与启动清理（含 archive id 与原因），便于事后确认某本书为什么晚出现。
- **FR-015**: 门禁 MUST 有自动化测试覆盖：D1（读写同一可见性域）、D2（访客分组 extend）、D3（阅读页三端点）、D4（索引写入前就已隐藏）、D5（列表接口）、D7（二次插件 pass）、D8（tank 成员）、崩溃清理与插件失败发布。

### Key Entities

- **Archive（档案）**：书库里的一本书，关键属性 `id`、`file`（最终字节路径）、`tags`、`date_added`；在 cataloguing 时写入。
- **Publication State（发布状态）**：某个 archive id 是否已对用户可见；属性包括 `archive_id`、`pending|published`、进入 pending 的时间、本次 ingest 尚未完成的插件 pass 数。由入库流程创建，由发布动作或启动收敛终结。
- **Metadata Plugin Run（插件执行）**：一次 ingest 中对某个档案串行执行的全部已启用 metadata 插件；它的结束（无论成败）是发布信号。

## Success Criteria

### Measurable Outcomes

- **SC-001**: 在「已 cataloguing、插件延迟 ≥30 秒」的窗口内，对 User Story 1 列出的全部搜索入口各轮询 20 次（管理员与访客、分组与未分组），pending 档案命中数为 **0**。
- **SC-002**: 同一窗口内，metadata/files/page 三个端点 100% 不可用（404），返回字节数为 0。
- **SC-003**: 最后一个插件返回后 1 秒内，同一搜索查询命中该档案，metadata/files/page 全部成功且 metadata 含插件标签。
- **SC-004**: 模拟崩溃并重启后 60 秒内，0 个档案处于隐藏态；dev 库现有 233 个已发布档案在改动前后可见集合完全一致。
- **SC-005**: 插件报错、无插件、重复下载三种路径各跑一次，档案最终都可见，且没有已发布档案被隐藏。
- **SC-006**: 自动化测试能在移除任一修复点时失败（至少覆盖 D1、D2、D3、D4、D5、D7、D8 与启动清理）。

## Assumptions

- 单实例部署，Redis 同时是档案记录、搜索索引与门禁状态的唯一存储；没有跨实例的可见性同步需求。
- 「完全入库」= 记录已保存 + 字节已在最终路径 + 本次 ingest 触发的所有 metadata 插件 pass 已返回；插件失败视为完成。
- metadata 插件是 best-effort 的标签增强，失败不应阻塞发布（与现状一致）。
- 访客模式可能开启（dev 实测 `guestmode=1`），因此门禁必须覆盖 `guest_visitor` 的分类收窄路径，不能只按管理员路径设计。
- 扫描器 watcher / full scan 不跑 metadata 插件，其「入库即可见」是既有行为，不在本 feature 内。
- 本 feature 不改变下载队列、插件执行顺序、搜索排序或索引结构，只改变「何时允许被发现」。

## Out of Scope

- 重做下载队列、插件执行框架或搜索架构。
- 为已发布档案提供「重新隐藏」能力或 retroactive 清理。
- 扫描器 watcher / full scan 的可见性语义。
- 在 pending 期间提供进度提示（例如「正在整理标签」的占位卡）；需求是「不出现」，不是「灰显」。
- 多实例/多节点的分布式可见性协调。

## Review Checklist

- [x] 用户故事可独立测试、按优先级排列（P1 搜索、P1 阅读页、P2 失败/崩溃、P3 其它入库路径）
- [x] 功能需求可验证（FR-001…FR-015 均对应至少一个验收场景或成功标准）
- [x] 成功标准可度量（轮询次数、秒数、命中数、档案集合对比）
- [x] 无明显待澄清项（`NEEDS CLARIFICATION` 为 0）
- [x] 范围边界明确（扫描器路径、已发布档案、进度占位均排除）
- [x] 现状缺陷有可复核的证据（文件:行号 + dev 实测结果）；这是本 feature 的输入，不是实现细节的越界

**Note**: 本 spec 的「现状与缺陷证据」章节刻意保留了文件路径与行号，因为用户明确要求把现在查出的问题记录进 spec；需求与验收标准本身仍按可观察行为描述，不绑定具体实现。
