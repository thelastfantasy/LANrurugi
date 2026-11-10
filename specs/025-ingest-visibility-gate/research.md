# Phase 0 Research: 入库可见性门禁

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Date**: 2026-11-10

本 feature 的「研究」不是选型，而是把 `b1723b6` 为什么没生效逐条钉死，再为每条缺陷选定最小修复面。证据来自源码 + dev 容器实机（HEAD `5784187`，二进制 `657ebf4`，档案 `fd3043a88275bfbfcd8ae091c8cc4b304166448e`）。

## R1. pending 状态放在哪个 Redis 逻辑库

**Decision**: archive DB（DB 0），键 `LANRURUGI_PENDING_VISIBILITY:<id>`，值 = 写入时的 Unix 毫秒（仅供排查，不参与判断）。

**Why**: `RedisDbs` 把 legacy 的 5 个逻辑库拆成独立连接池（`crates/lanrurugi-storage/src/redis.rs`：archive=0、minion=1、config=2、search=3、metrics=4）。`SearchEngine` 只持有 `archive_pool`/`search_pool`（`crates/lanrurugi-search/src/engine.rs:131`），搜索的 `pending_visibility` 走 `archive_conn`；API 的档案仓也绑 DB 0（`crates/lanrurugi-api/src/state.rs` 的 `Repositories::new`）。原实现却用 `state.redis.config`（DB 2）写标记——各逻辑库 key 空间隔离，engine 永远读不到（D1）。dev 实测：只写 DB 2 时搜索照常命中；`MONITOR` 显示应用执行的是 `[0 ...] KEYS LANRURUGI_PENDING_VISIBILITY:*`。

**Alternatives considered**:
- 给 engine 加 config 池、继续写 DB 2：能把读端改对，但「谁都能读」的语义被拆到两处，后续任何新读者都要记得跨库；且 engine 的构造签名要改。
- 用内存 `DashSet`：重启丢失、与「崩溃后仍要能清理」的目标冲突，多实例更不成立。
- 新开一个逻辑库：legacy 的 5 库划分是兼容契约的一部分，凭空加库只会增加部署认知负担。

## R2. 标记写在 cataloguing 之前还是之后

**Decision**: 之前。`catalogue_staged_file` 在 `content_lock` 与 `Unchanged` 早退之后、`rename(incoming, dest)` 之前写标记。

**Why**: `ingest_file_with_policy` 内部的 `catalogue_new_archive`（`crates/lanrurugi-scanner/src/pipeline.rs:491`）会 `archives.save` → 写旁车 → `index_new_archive` → 生成缩略图再 save，返回后才轮到原实现的 `mark_pending_visibility`（`ingest.rs:458`）。索引写入与标记之间有一个必然窗口（D4），即使 D1/D2 都修好，这段窗口仍能被搜到。`catalogue_staged_file` 在 rename 前已用 `size_aware_id(&incoming)` 算出 id；该函数是「前 512KB + 文件长度」的 SHA-1（`crates/lanrurugi-storage/src/id.rs:71`），与路径/文件名无关，所以先标记、后 rename 不会错 id。

**Alternatives considered**:
- 在 `catalogue_new_archive` 内部、`index_new_archive` 之前标记：更靠近索引写入点，但 scanner 也走 `catalogue_new_archive`（不跑插件），会把「扫描入库」也变成 pending，需要额外的「谁会发布」判断，违反「扫描器路径不变」的边界。
- 在索引写入后、插件前标记：即现状，D4 仍在。

## R3. 发布信号与「最后一次插件 pass」

**Decision**: 显式 `publish`。`catalogue_staged_file` 在它自己触发的插件跑完后 publish；新增 `defer_publication` 参数，上传路径传 `true`，由 `upload.rs` 在第二次插件 pass（`upload.rs:902`）返回后 publish。

**Why**: 上传的流程是「入库（含第一遍插件）→ 写用户标题/标签 → 再跑一遍插件叠加」（`upload.rs:867-910`，注释说明这是 legacy `Model::Upload.pm` 的真实顺序）。如果入库结束时就把标记清掉，第二遍插件期间档案已可见（D7）。`defer_publication` 把「谁负责发布」显式交给知道完整流程的调用方，而不是靠猜。

**Alternatives considered**:
- 引用计数（每次插件 pass +1/-1，归零发布）：能覆盖任意嵌套，但两次 pass 之间计数会归零，需要「延迟删除」或「调用方在入库前先 +1」，复杂度高于收益。
- 把 publish 全部上移到各调用方：8 个调用点漏一个就永久隐藏（直到重启），比一个显式 bool 更脆。
- 让 `run_enabled_metadata_plugins_on_archive` 自己管发布：它也被订阅的「源变更后重新抓元数据」调用（`subscriptions/runner.rs:902`），那是已发布档案的再增强，不能隐藏（FR-010）。

## R4. TTL 还是显式清理 + 启动收敛

**Decision**: 去掉 TTL（删除 `PENDING_VISIBILITY_TTL_SECS`）；publish 失败重试 3 次；服务启动时清除全部残留 pending。

**Why**: TTL 的两个方向都错：正常插件跑超过 1 小时会被提前放行（提前可见）；进程崩溃后 1 小时内档案隐藏、之后带半套标签冒出来（不可控）。显式清理 + 启动收敛把「何时可见」变成确定事件；崩溃后的选择是「发布（带已有标签）」而不是「继续等」，因为崩溃的 ingest 已经无法补跑插件。

**Alternatives considered**:
- 保留 TTL 但设很长（24h）：仍是时间驱动的可见性，违背 FR-008；且库里会长期残留键。
- 周期性扫描「孤儿 pending」：本次不做；publish 失败已重试，Redis 不可用时搜索本身也不可用，重启收敛即可兜底。

## R5. 阅读页在 pending 期间返回什么

**Decision**: `metadata` / `files` / `page`（以及 thumbnail、download 等只读入口）统一 404，响应体不透露档案存在。保留现有「空 `file` → 409」作为历史兜底。

**Why**: 用户要求「阅读页 URL 有效化」发生在完成之后；404 与 `guest_scope_denies` / 不存在档案的既有行为一致，不给出「这个 id 存在但还在处理」的旁路信息。dev 实测：标记写入 DB 0 后 metadata 200、files 200、page 200（360096 字节），说明当前完全没有门禁（D3）。

**Alternatives considered**:
- 409「仍在入库」：对直接粘贴 URL 的用户更友好，但等于确认 id 存在；与 guest 访问不存在的 404 语义不一致。
- 前端加 pending 状态：需要新字段/新接口，且不阻止直接 API 访问；后端门禁是唯一可靠点。

## R6. 过滤放在搜索管线的哪一步

**Decision**: 候选集组装的最后一步，一次性 `retain`（在 `restrict_to_archive_ids` 的 `extend` 与 `retain(allowed_set)` 之后）。

**Why**: 原实现的 retain 在 `engine.rs:473`，之后 `:490-506` 又为访客 `filtered.extend(allowed...)`，把 pending id 加了回来（D2）。dev 复现完全吻合：`/api/search`（默认分组、未登录）命中，`groupby_tanks=false` 与 `/api/search/archives` 返回空。把过滤放最后，未来再加候选集变更也不会重蹈覆辙。

**Alternatives considered**:
- 只在 extend 后再 retain 一次、前面保留：两次都对，但读代码的人容易以为前面那次是唯一保证；最终一次 + 注释即可。
- 在结果分页后再过滤：会破坏 `recordsFiltered`/分页（007 已明确否决同类做法）。

## R7. mark/publish 失败时 fail open 还是 fail closed

**Decision**: mark 失败 → 该次入库失败（返回错误）；publish 失败 → 重试 + 启动收敛；`is_pending` 在 Redis 错误时按 pending 处理。

**Why**: 档案记录与索引本身就在 Redis 里，mark 写不进去通常意味着 Redis 不可用，此时入库本来也成功不了；若「跳过标记继续入库」，一旦 Redis 恢复，档案就会以未完成状态出现——正是用户要消除的。publish 失败则相反：宁可多隐藏一会儿，也不能提前可见；启动收敛保证不会永久隐藏。

**Alternatives considered**:
- mark 失败也继续入库并记 warning：正是原实现的 best-effort 语义（D6），会让门禁在 Redis 抖动时静默失效。

## 调查方法（可复核）

- 源码：`git log -S PENDING_VISIBILITY` 只有 `b1723b6`；全仓 8 处引用、0 个测试。
- 实机：dev 容器 `lrr-dev`（host network，Redis `127.0.0.1:16379`），未登录请求走 `guest_visitor`（`LRR_CONFIG guestmode=1`，`procedure.rs:146`），分类 `SET_1787832596`/`SET_1784896712` 对访客可见。
- 对照实验：同一 `source:` 过滤在无标记时命中 `fd3043…`；写入 DB 2 无效果；写入 DB 0 后 grouped 仍命中、ungrouped 为空；metadata/files/page 全 200。
- 反例排除：`filter=zzzznonexistentzzz` 无标记时 grouped 为空、`recordsFiltered=0`，排除「分组结果钉住首条」；ID2/ID3 的早先对照因过滤本身未命中而作废。
