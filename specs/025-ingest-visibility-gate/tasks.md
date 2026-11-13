# Tasks: 入库可见性门禁

**Input**: Design documents from `specs/025-ingest-visibility-gate/`
**Prerequisites**: plan.md（含 2026-11-13 Addendum）、spec.md、research.md（R1–R8）、data-model.md、contracts/visibility-contract.md、quickstart.md

**Tests**: FR-015 明确要求自动化测试，每个故事都带测试任务。真实 Redis 测试用 `LANRURUGI_TEST_REDIS_URL`（本机 `redis://127.0.0.1:16380`）。

行号以 `516e9c8` 为准。

## Phase 1: Setup

- [ ] T001 确认基线：`/api/archives` 与 `/api/search` 在 dev 库上的可见 id 集合存档到 `specs/025-ingest-visibility-gate/baseline-ids.txt`（SC-004 对比用；dev 容器不可用时在 Implementation Record 说明并以单测替代）

## Phase 2: Foundational（所有故事的前置）

- [x] T002 在 `crates/lanrurugi-storage/src/keys.rs` 把 pending 标记改为单个 SET 键 `PENDING_VISIBILITY_SET = "LANRURUGI_PENDING_VISIBILITY"`，保留 `PENDING_VISIBILITY_PREFIX` 仅供启动时清理遗留单键，删除 `PENDING_VISIBILITY_TTL_SECS` 与 `pending_visibility_key`
- [x] T003 新建 `crates/lanrurugi-api/src/visibility.rs`：`mark_pending`（`SADD`，失败返回 Err）、`publish`（`SREM`，重试 3 次后 `error!`）、`is_pending`（`SISMEMBER`，Redis 错误按 pending 处理）、`pending_ids`（`SMEMBERS`）、`clear_all_pending`（`DEL` SET + 对 DB 0/DB 2 `SCAN` 删除遗留 `LANRURUGI_PENDING_VISIBILITY:*`），全部走 `state.redis.archive`；mark/publish/clear 写 `tracing::info!`（FR-014）；在 `crates/lanrurugi-api/src/lib.rs` 注册模块
- [x] T004 `crates/lanrurugi-search/src/engine.rs:188` `pending_visibility()` 改为对 `archive_conn` 执行 `SMEMBERS`（不再 `KEYS`）
- [x] T005 [P] `crates/lanrurugi-api/src/visibility.rs` 单测（真实 Redis）：mark→is_pending→pending_ids→publish 往返；clear_all 同时清掉 SET 与 DB 0/DB 2 的遗留单键；不可达 pool 时 mark 返回 Err、is_pending 返回 true

**Checkpoint**: 状态层就绪，行为尚未改变。

## Phase 3: User Story 1 — 下载未完成时搜索不到（P1）

**Goal**: 所有搜索入口（含访客、分组/不分组、suggest/random/ids/OPDS）都看不到 pending 档案，tank 卡片不暴露 pending 成员。

**Independent Test**: 直接把一个已存在档案的 id `SADD` 进 SET，各入口查询 0 命中；`SREM` 后立即命中。

- [x] T006 [US1] `crates/lanrurugi-search/src/engine.rs`（`search` 内 `:471-506`）：把 pending 过滤移到 `restrict_to_archive_ids` 的 `extend` 与 `retain(allowed_set)` 之后，作为候选集最后一步（D2）；早期那次可保留，最终那次必须有；注释写明「候选集最终必须是已发布集合的子集」
- [x] T007 [US1] `crates/lanrurugi-search/src/engine.rs` 的 `fold_tanks`（`:706-739` 附近）：确认 `matched` 来自最终过滤后的集合，补注释；只有 pending 成员的 tank 不出现（D8）
- [x] T008 [US1] `crates/lanrurugi-search/src/engine.rs` `search_exists`（`:217`）与 `:237/:246` 两处：沿用 `SMEMBERS` 结果过滤，核对与 T006 同一不变量
- [x] T009 [P] [US1] `crates/lanrurugi-search/src/engine.rs` 测试（`test_pools`）：管理员分组/不分组、访客 `restrict_to_archive_ids` 含 pending id 且 `groupby_tanks=true`、tank 成员中有 pending 时 `member_matches` 不含它、只有 pending 成员的 tank 不出现、`SREM` 后同查询命中（D1、D2、D8、FR-007）
- [x] T010 [US1] `crates/lanrurugi-api/src/search.rs` 与 `crates/lanrurugi-api/src/opds.rs`：核对 `/search`、`/search/archives`、`/search/ids`、`/search/suggest`、`/search/random`、OPDS 全部经 `engine::search`，无绕过；在 `search.rs` 的 `build_params` 附近补一行注释指向 T006 的不变量

**Checkpoint**: US1 可单独验证。

## Phase 4: User Story 2 — 下载未完成时阅读页 URL 无效（P1）

**Goal**: pending 期间阅读与下载类只读接口一律 404，枚举接口不列出。

**Independent Test**: `SADD` 一个已存在 id，请求 metadata/files/page 等端点得 404 且无字节；`SREM` 后 200。

- [x] T011 [US2] `crates/lanrurugi-api/src/archives.rs`：在 `get_archive_metadata`（`:408`）、`get_files`（`:4494`）、`serve_page`（`:3393`，覆盖 `get_page` 与 OPDS 页流）、`get_page_dimensions`（`:4571`）、`get_archive_thumbnail`（`:1883`）、`get_page_thumbnails`（`:4765`）、`download_archive`（`:978`）、`download_link`（`:964`）的访客范围检查之后、取档案之前加 `visibility::is_pending` → `not_found`
- [x] T012 [US2] `crates/lanrurugi-api/src/archives.rs` `list_archives`（`:304`）、`untagged_archives`（`:331`）：取一次 `pending_ids` 后过滤再排序/分页（D5、FR-013）
- [x] T013 [P] [US2] `crates/lanrurugi-api/src/archives.rs` 测试（真实 Redis + 临时书库，沿用 `plugins.rs::test_state()` 范式）：pending 时上述端点 404 且响应体不含标题/页字节；publish 后 200；两个列表接口不含 pending（D3、D5）

**Checkpoint**: US1 + US2 即用户报告的完整修复。

## Phase 5: User Story 3 — 失败与崩溃不会留下永久隐藏的档案（P2）

**Goal**: 标记在索引写入之前就位；插件失败、IO 失败、Rejected 都会发布；崩溃遗留由启动收敛清除；已发布档案从不被重新隐藏。

**Independent Test**: 插件报错 / 无插件 / 重复下载 / 重启遗留标记，四种情况最终都可见，已发布档案全程可见。

- [x] T014 [US3] `crates/lanrurugi-api/src/download_manager/ingest.rs` `catalogue_staged_file`：删除旧的 `mark_pending_visibility`/`clear_pending_visibility`（`:253-280`）；在 content lock 与 `Unchanged` 早退之后、`rename(incoming, dest)`（`:390`）之前调用 `visibility::mark_pending`，失败则清理 `incoming`、`put_bytes_back` 并返回错误（D4、I2、I8）；「记录缺字节被修复」分支（`:356-379`）是已发布档案，不 mark（I4）
- [x] T015 [US3] 同函数：`ingest_file_with_policy` 返回 `Rejected` 时 publish；cataloguing/IO 失败路径 publish 后再返回；插件返回后 publish（原 `:564`），包在 `if !defer_publication` 内（FR-006、FR-011）
- [x] T016 [US3] 同文件：`catalogue_staged_file` 与 `ingest_downloaded_file`（`:223`）增加参数 `defer_publication: bool` 并透传；`resolve_overwrite`（`:728`）、`resolve_rename`（`:751`）、`adopt_pending_conflict`（`:118`）、启动接管/旧队列收编（`:888-1076` 各调用）传 `false`
- [x] T017 [US3] Rekey 路径核对：`ingest_file_with_policy` 返回 `Rekeyed` 时，新 id 处于 pending、旧 id 不被 mark（US3 场景 4）；在 `ingest.rs` 对应分支补注释
- [x] T018 [US3] `crates/lanrurugi-server/src/main.rs:747`：`repair_zombie_archives` 之后、下载队列恢复之前调用 `lanrurugi_api::visibility::clear_all_pending`，`info!` 清除数（FR-012、I7）
- [x] T019 [P] [US3] `crates/lanrurugi-api/src/download_manager/ingest.rs` 测试（真实 Redis + 临时书库）：无启用插件 → 入库后立即不在 SET；插件报错 → 仍发布；`Unchanged` 重复入库 → 已发布档案从未进入 SET；mark 失败（不可达 pool）→ 入库失败且书库无新文件
- [x] T020 [P] [US3] 启动收敛测试：预置 SET 成员与 DB 0/DB 2 遗留单键，调用 `clear_all_pending` 后全部清除（放在 `visibility.rs` 测试内，与 T005 同文件需串行）

## Phase 6: User Story 4 — 上传 / 共享导入 / 订阅路径（P3）

**Goal**: 上传的第二次插件 pass 结束前不发布；共享导入与订阅走同一门禁；重复检测与推荐不泄漏 pending。

**Independent Test**: 上传与共享导入在插件窗口内不可见/不可读，结束后可见。

- [x] T021 [US4] `crates/lanrurugi-api/src/upload.rs:866`：`ingest_downloaded_file(..., defer_publication = true)`；在 `:901` 第二次插件 pass 返回后 `visibility::publish`；两次 pass 之间的每个提前返回（含错误）先 publish（D7）
- [x] T022 [US4] `crates/lanrurugi-api/src/share_net/import.rs:1083` 传 `defer_publication = false`；核对订阅只经下载队列入库、`subscriptions/runner.rs` 的再增强只作用于已发布档案（不 mark）
- [x] T023 [US4] `crates/lanrurugi-api/src/duplicates/mod.rs:101`：重复扫描聚组前过滤 pending（plan Addendum 第 2 条）
- [x] T024 [US4] `crates/lanrurugi-api/src/recommend.rs`（`:326-347` 读 Top-N 处）：与「已删除 id」过滤一并去掉 pending（plan Addendum 第 3 条）
- [x] T025 [P] [US4] 测试：上传在第二次 pass 期间 `is_pending` 为真、结束后为假（在 `upload.rs` 测试或以 `defer_publication=true` 调 `catalogue_staged_file` 断言不发布）；重复扫描结果不含 pending id

## Phase 7: Polish & Cross-Cutting

- [x] T026 全仓 grep `PENDING_VISIBILITY_TTL_SECS|pending_visibility_key|mark_pending_visibility|clear_pending_visibility|KEYS LANRURUGI_PENDING` 无残留
- [x] T027 `mise run fmt` / `mise run check-crate -- lanrurugi-storage lanrurugi-search lanrurugi-api lanrurugi-server` / `mise run clippy`
- [x] T028 `mise run test-crate -- lanrurugi-search lanrurugi-api`（detached，真实 Redis），再 `mise run test` 全量
- [ ] T029 quickstart.md 场景（dev 容器）：能跑就跑并记录；不能跑的写明原因，以单测覆盖说明替代
- [x] T030 `docs/api/sidecar/` 若有对上述端点的说明，补「入库未完成时返回 404」；`mise run docs-api -- --check`
- [x] T031 在本文件末尾写 Implementation Record：每个 D1–D8 的修复点与对应测试名、哪些只在单测里验证、与 plan 的偏差

## Dependencies & Execution Order

- Phase 2（T002–T005）阻塞所有故事。
- US1（T006–T010）与 US2（T011–T013）互不依赖，可并行。
- US3 的 T014–T016 改入库签名，US4 的 T021–T022 依赖 T016。
- T023、T024 只依赖 Phase 2。
- Polish 在全部故事之后。

## Parallel Examples

- Phase 2 之后：T006（engine）与 T011（archives）可同时进行。
- 测试任务 T009、T013、T019、T025 分属不同文件，可并行编写。

## Implementation Strategy

MVP = Phase 2 + US1 + US2（用户报告的两个现象）。随后 US3（失败与崩溃不留隐藏档案）必须在同一次发布里完成，否则「写标记」会把失败的入库永久隐藏。US4 收尾。整体在 beta6 发布前完成。

## Implementation Record（2026-11-13）

基于 `516e9c8`。D1–D8 的修复点与测试：

| 缺陷 | 修复 | 测试 |
|---|---|---|
| D1 写错逻辑库 | 标记改为 DB 0 的 SET `LANRURUGI_PENDING_VISIBILITY`，读写都走 `state.redis.archive`（`visibility.rs`）；engine 用 `SMEMBERS` | `engine::tests::pending_archives_are_hidden_from_every_search_shape`、`visibility::tests::mark_and_publish_round_trip` |
| D2 访客分组 extend 放回 | pending 过滤移到 `restrict_to_archive_ids` 的 extend/retain 之后，并在 `eval_expr` 之后再过滤一次（`engine.rs` `search`） | 同上测试的 guest grouped / ungrouped 两种形态 |
| D3 阅读页无门禁 | `archives.rs::read_denied` = 访客范围 ∪ pending，用于 metadata、files、page（`serve_page`，含 OPDS 页流）、page-dimensions、thumbnail、page thumbnails（含 tank 成员）；download / download-link 单独判 pending | `archives::tests::a_pending_archive_is_unreadable_and_unlisted_until_published` |
| D4 标记晚于索引 | `catalogue_staged_file` 在 content lock 与已存在早退之后、`rename(incoming, dest)` 之前 `mark_pending`；失败则入库失败并放回字节 | `ingest::tests::ingest_publishes_at_the_right_moment_and_never_hides_an_existing_archive` |
| D5 列表接口 | `list_archives`、`untagged_archives` 过滤 `pending_ids` | `archives::tests::…_until_published` |
| D6 TTL 与崩溃 | 无 TTL；`PendingGuard` 在 future 被取消时 spawn publish；启动在 `repair_zombie_archives` 之后、队列恢复之前 `clear_all_pending`（同时删除 DB 0/DB 2 的旧单键） | `visibility::tests::startup_publishes_everything_and_drops_the_old_marker_keys`、`an_unreachable_redis_fails_closed` |
| D7 上传二次 pass | 新增 `ingest_uploaded_file`（`defer_publication=true`）；`upload.rs` 在拿到结果后立即建 `PendingGuard`，第二次插件 pass 后 publish | ingest 测试的 deferred 分支 |
| D8 tank 成员 | 最终候选集与 `eval_expr` 结果都排除 pending，`fold_tanks` 的 `matched` 因而不含 pending；只有 pending 成员的 tank 不出现 | engine 测试的 `member_matches` 断言 |

plan Addendum 的补充：重复扫描跳过 pending（`duplicates/mod.rs::run_scan`）；推荐把 pending 加入排除集（`recommend.rs::get_recommendations`，缓存与全库两条路径都生效）。这两处没有单独的测试。

**与 plan 的偏差**

- 标记形状由「每档案一个键」改为一个 SET（research R8），`KEYS` 不再出现在搜索路径上。
- 上传没有给 `ingest_downloaded_file` 加参数，而是新增 `ingest_uploaded_file`；其余调用点签名不变。
- 新增 `PendingGuard`：plan 只写了「错误路径前先 publish」，没有覆盖请求被取消（客户端断开）这种情况。
- `ingest_file_with_policy` 返回的 id 与预先隐藏的 id 不同时（理论上不会发生，pipeline 由同一份字节算 id），记 `warn` 并发布被隐藏的 id。

**验证范围**

- `mise run check-crate -- lanrurugi-storage lanrurugi-search lanrurugi-api lanrurugi-server` 通过。
- `mise run test-crate -- lanrurugi-search`（59 passed）、`-- lanrurugi-api`（678 + 7 passed），新增 6 个测试全部实际执行（未因缺 Redis 跳过）。
- 第一次合跑三个 crate 时被 systemd-oomd 以内存压力杀掉（exit 137，非代码问题），改为逐个 crate、等 PSI 回落后跑。
- **未做**：T001 基线 id 集合存档、T029 dev 容器 quickstart 手测（需要重建 dev 镜像并配慢插件）、全 workspace `mise run test`。这三项留给发版前的验收；SC-001/SC-002 目前由单测覆盖而非真实下载窗口。
- T030：受影响端点没有 `docs/api/sidecar/` 条目（来自 legacy OpenAPI），无需改文档。
