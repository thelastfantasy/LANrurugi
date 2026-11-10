# Implementation Plan: 入库可见性门禁

**Branch**: `feat/share-bind-interface`（沿用当前分支，不新建） | **Date**: 2026-11-10 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/025-ingest-visibility-gate/spec.md`

## Summary

把「档案可见」的开关从「记录一写入就可见」改成「本次 ingest 的全部 metadata 插件都返回后才可见」。技术上不新增存储：沿用 `LANRURUGI_PENDING_VISIBILITY:<id>` 键，但

1. 改写到 **archive 逻辑库（DB 0）**——search engine 与 API 都从该池读；
2. 标记提前到 **cataloguing 之前**（id 已由 `size_aware_id` 算出）；
3. 搜索候选集的 pending 过滤挪到 **所有 extend/retain 之后**（修访客 `restrict_to_archive_ids` 二次放行）；
4. 阅读页/文件/页接口全部加 404 门禁；
5. 去掉 TTL，改为显式 publish + 启动收敛，避免「1 小时后自己冒出来」；
6. 上传的第二次插件 pass 用 `defer_publication` 延后发布，避免上传档案在叠加用户标签时提前出现。

## Technical Context

**Language/Version**: Rust（workspace，edition 随现有 crate；无新依赖）

**Primary Dependencies**: axum、tokio、deadpool-redis、redis-rs（均已存在）；search 侧 `lanrurugi-search`；无新增 crate

**Storage**: Redis（`RedisDbs`：archive=DB 0、minion=1、config=2、search=3、metrics=4）。本 feature 只在 DB 0 增加瞬态键，不改任何持久化 schema

**Testing**: `cargo test`（经 `mise run test-crate` / `mise run test`）；`LANRURUGI_TEST_REDIS_URL` 驱动的真实 Redis 测试（`lanrurugi-search/src/engine.rs` 的 `test_pools()`、`lanrurugi-api/src/plugins.rs` 的 `test_state()` 是现成范式）；dev 容器手工复现作为最终验收

**Target Platform**: Linux 服务器（单进程，Tokio runtime）

**Project Type**: Web service（Rust 后端 + React 前端；本 feature 不改前端）

**Performance Goals**: 每次搜索多一次 `KEYS`/`SCAN`（已有）；每个阅读页请求多一次 `EXISTS`；入库多两次 Redis 写。相对一次下载/一次插件 subprocess 可忽略。发布后第一次请求即可见（无轮询延迟）

**Constraints**: 不改既有 API 的响应结构（Principle II）；不改变已发布档案行为；不引入新后台进程；扫描器 watcher/full scan 语义不变

**Scale/Scope**: 单实例；档案量级为现有 dev 库 233、目标用户数万；pending 集合通常 0–数个

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| 原则 | 结论 | 说明 |
|------|------|------|
| I. Legacy Data & User-Trust Compatibility | PASS | 不改档案 id 算法、不迁移既有记录；pending 键是瞬态命名空间，启动清理保证不残留。已发布档案的可见集合不变（SC-004 显式回归） |
| II. API Contract Fidelity | PASS（需说明） | 路径与响应结构不变；pending 期间 metadata/files/page 由 200 变 404——这些档案此前对任何客户端都「尚不存在」，没有可依赖的既有行为。`/api/search` 结果集只是少掉尚未完成的条目 |
| III. Resource-Conscious Single-Process | PASS | 全部是既有 Redis 池上的短命令，无 `spawn_blocking`、无 rayon、无新进程；启动收敛复用 `repair_zombie_archives` 的同一收敛阶段 |
| IV. Sandboxed Plugin Extensibility | PASS | 不碰插件运行时；只是等待现有插件调用返回 |
| V. Secrets & Network Trust Boundaries | N/A | 无网络/密钥变更 |
| VI. Phased Scope Discipline | PASS | Phase 1 的入库正确性修复，不涉及 Phase 2 |
| VII. Frontend Engineering Discipline | PASS | 无前端改动；阅读页在 metadata 404 时已有错误分支（`Reader.tsx:1724`），无需改 UI |

## Project Structure

### Documentation (this feature)

```text
specs/025-ingest-visibility-gate/
├── plan.md                          # 本文件
├── spec.md                          # 需求 + 现状缺陷证据（D1–D8）
├── research.md                      # Phase 0：7 个决策与备选
├── data-model.md                    # Phase 1：发布状态机与不变量
├── contracts/
│   └── visibility-contract.md       # Phase 1：受影响的端点语义契约
├── quickstart.md                    # Phase 1：dev 容器复现/验收步骤
├── checklists/requirements.md       # spec 质量清单
└── tasks.md                         # /speckit-tasks 生成（本命令不产出）
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-storage/src/
│   └── keys.rs                      # pending 键名；删除 TTL 常量
├── lanrurugi-api/src/
│   ├── visibility.rs                # 新增：mark / publish / is_pending / pending_ids / clear_all
│   ├── archives.rs                  # 阅读页三端点 404；thumbnail/download 同门禁；两个 list 过滤
│   ├── download_manager/ingest.rs   # 标记提前；失败/重复清理；defer_publication
│   ├── download_manager.rs          # resolve_overwrite/resolve_rename 传 false
│   ├── upload.rs                    # 传 defer=true，第二次插件 pass 后 publish
│   ├── share_net/import.rs          # 传 false
│   ├── lib.rs                       # 挂 visibility 模块
│   └── search.rs                    # 访客 restrict 与过滤顺序的注释/调用点核对
├── lanrurugi-search/src/
│   └── engine.rs                    # 最终 retain 移到 restrict extend 之后；测试
└── lanrurugi-server/src/
    └── main.rs                      # 启动收敛：清除崩溃遗留 pending
```

## Phase 0: Research (summary)

详见 [research.md](./research.md)。关键决策：

1. **状态存放**：archive DB（DB 0）。理由：search engine 只有 archive/search 两个池，API 的档案仓也在 DB 0；放 config DB 正是 D1 的根因。备选（config DB + 给 engine 加池 / 内存）均被否。
2. **标记时机**：cataloguing 之前。理由：`catalogue_new_archive` 内部就写索引，之后标记必然留窗口（D4）；id 在 rename 前已由 `size_aware_id`（路径无关的「前 512KB + 长度」）算出。
3. **发布信号**：显式 `publish`，由 ingest 在插件返回后调用；上传路径 `defer_publication=true`，由 upload 在第二次插件 pass 后调用。不用 TTL。
4. **崩溃恢复**：启动时清除全部残留 pending（在 `repair_zombie_archives` 之后）。理由：崩溃的 ingest 已经无法补跑插件，发布（带半套标签）优于永久隐藏；与 FR-012 一致。
5. **阅读页状态码**：统一 404，不泄露「这个 id 存在但还没好」。保留现有空 `file` 的 409 作为「记录先于字节」的历史兜底（本 feature 后该窗口已消失，但扫描器等路径仍可能产生）。
6. **过滤位置**：候选集组装完成后一次性 retain；不依赖「某个分支已经过滤过」。备选（在各分支各 retain 一次）留下 D2 同类缺口。
7. **失败语义**：mark 失败 → 该次入库失败（fail closed）；publish 失败 → 重试 3 次 + 启动收敛兜底。

## Phase 1: Design

- [data-model.md](./data-model.md)：发布状态（pending/published）、转换、不变量（I1–I8）。
- [contracts/visibility-contract.md](./contracts/visibility-contract.md)：受影响的 8 个端点/入口在 pending 与 published 两种状态下的行为。
- [quickstart.md](./quickstart.md)：dev 容器里用慢插件复现「搜索/阅读页提前可见」，再验证修复。

## Implementation Breakdown

### M1 — 可见性状态层（无行为变化）

- 新增 `crates/lanrurugi-api/src/visibility.rs`：
  - `pub async fn mark_pending(state: &AppState, id: &ArchiveId) -> Result<(), VisibilityError>`：`SET pending_visibility_key(id) <unix_ms>`（无 TTL），失败返回 Err。
  - `pub async fn publish(state: &AppState, id: &ArchiveId)`：`DEL`，失败重试 3 次（短退避），最终失败 `tracing::error!`。
  - `pub async fn is_pending(state: &AppState, id: &ArchiveId) -> bool`：`EXISTS`；Redis 错误按 `true` 处理（fail closed；此时档案本身也读不出来）。
  - `pub async fn pending_ids(state: &AppState) -> HashSet<String>`：`SCAN MATCH prefix*`（不用 `KEYS`，避免大库阻塞；engine 侧保留现有 `KEYS` 或一并改 `SCAN`）。
  - `pub async fn clear_all_pending(state: &AppState) -> usize`：启动收敛，返回清除数并 `tracing::info!`。
- `crates/lanrurugi-storage/src/keys.rs`：保留 `PENDING_VISIBILITY_PREFIX`/`pending_visibility_key`，删除 `PENDING_VISIBILITY_TTL_SECS`（无其他使用者）。
- `crates/lanrurugi-api/src/lib.rs` 注册模块。

### M2 — 搜索管线（D1、D2、D4、D8）

- `crates/lanrurugi-search/src/engine.rs`：
  - `pending_visibility()` 改用 `SCAN`（与 M1 一致），仍从 `archive_conn` 读。
  - 把 `:473` 的 `filtered.retain(...)` **移动**到 `:490-506` 的 `restrict_to_archive_ids` 块之后（`retain(allowed_set)` 之后），成为候选集最后一步；保留一次早期 retain 也无害，但最终那次必须有。
  - `fold_tanks` 的 `matched` 因此天然不含 pending；补注释说明「成员命中集合必须是已发布集合的子集」。
  - `search_exists` 的过滤保持不变。
- `crates/lanrurugi-api/src/search.rs`：核对 `build_params` 的 `restrict_to_archive_ids` 只在 GuestVisitor 设置；`resolve_search_entry_with_mode` 保持只判空 `file`（搜索页拿不到 pending id，不再增加 N+1）。

### M3 — 阅读页与列表门禁（D3、D5）

- `crates/lanrurugi-api/src/archives.rs`：在上述每个 handler 的 `guest_scope_denies` 之后、取档案之前加 `is_pending` → `not_found`：
  - `get_archive_metadata`（:408）
  - `get_files`（:4470）
  - `serve_page`（:3377，同时覆盖 `get_page` 与 OPDS 页流）
  - `get_page_dimensions`、`get_archive_thumbnail`、`get_page_thumbnails`、`download_archive`、`download_link`（同一读路径，保持一致；`PUT`/`DELETE` 不动）
  - `list_archives`（:304）、`untagged_archives`（:331）：一次 `pending_ids`，过滤后再排序/分页。
- 不改 `get_archive_categories`/`get_archive_tankoubons`/`toc`（管理向读取，不构成发现/阅读入口）。

### M4 — 入库流程（D4、D6、D7）

- `crates/lanrurugi-api/src/download_manager/ingest.rs`：
  - 在 `content_lock` + `Unchanged` 早退之后、`rename(incoming, dest)` **之前** `mark_pending(...)`；失败即返回 `IngestDownloadError`（并清理 `incoming`）。
  - 删除 `:458` 的原标记；`ingest_file_with_policy` 返回 `Rejected` 时 `publish` 并返回；`Catalogued`/`Rekeyed` 继续。
  - cataloguing/IO 失败路径：`publish`（best-effort）后再返回错误。
  - 保留插件后 `publish`，但包在 `if !defer_publication` 里。
  - 新增参数 `defer_publication: bool`；`ingest_downloaded_file` 增加同名参数并透传。
- 调用点：
  - `download_manager.rs` 的 `resolve_overwrite`/`resolve_rename`（ingest.rs:119/697 等）→ `false`
  - `share_net/import.rs:1083` → `false`
  - `upload.rs`：`ingest_downloaded_file(..., defer=true)`；在 `:902` 第二次插件 pass 返回后 `visibility::publish`。若两次 pass 之间 upload 提前返回错误，`publish` 放在错误路径之前或用 `scopeguard` 式显式处理（Rust 无 async Drop，写成一个 `publish_after_upload` 局部 async 块，`?` 前先 publish）。
  - adoption/旧冲突路径（ingest.rs:827/879/930/1011）→ `false`。

### M5 — 启动收敛（D6）

- `crates/lanrurugi-server/src/main.rs`：`repair_zombie_archives(&state).await;` 之后调用 `lanrurugi_api::visibility::clear_all_pending(&state).await`，`tracing::info!(cleared = n, "cleared stale ingest-visibility markers")`。
- 注意顺序：在下载队列恢复之前（`serve()` 的队列启动在同一启动段），避免「刚发布又被恢复的 ingest 重新标记」的竞态；即使顺序反了，重新标记也只影响尚未完成的档案。

### M6 — 测试与验收

- `lanrurugi-search/src/engine.rs`（真实 Redis，沿用 `test_pools`）：
  - guest + `restrict_to_archive_ids` 包含 pending id + `groupby_tanks=true` → 不返回（D1/D2）。
  - `groupby_tanks=false` → 不返回。
  - tank 有 pending 成员 + 另一已发布成员命中 → tank 出现，但 `member_matches` 不含 pending（D8）。
  - 删除标记后同一查询命中（发布即时性）。
- `lanrurugi-api/src/archives.rs`（沿用 `plugins.rs::test_state()` 范式或新增最小 `AppState`）：
  - pending 时 metadata/files/page 均 404；publish 后 200（D3）。
  - `list_archives`/`untagged_archives` 不返回 pending（D5）。
- `lanrurugi-api/src/download_manager/ingest.rs`：
  - 无启用插件 → 入库后立即可见；
  - 插件返回错误 → 仍发布；
  - 重复入库（Unchanged）→ 不产生标记、已发布档案不被隐藏（D4/FR-010）。
  - 用 `LANRURUGI_TEST_REDIS_URL` + 临时 `plugins/custom/metadata/` fixture（或注入 fake plugin runner，若现有 seam 允许）。
- `lanrurugi-api/src/visibility.rs`：mark/publish/pending_ids/clear_all 的单测（真实 Redis）；mark 失败返回 Err 的分支用不可达 pool 验证。
- quickstart.md 的 dev 手工验收在实现后跑一遍，作为最终 sign-off。

## Risks & Mitigations

| 风险 | 缓解 |
|------|------|
| 访客路径之外还有别的「先过滤后并入」分支 | 最终 retain 放在候选集最后一步；测试覆盖 guest + grouped；代码注释写明不变量 |
| 上传第二次 pass 之间进程崩溃 → 档案隐藏 | 启动收敛发布；upload 错误路径显式 publish |
| Redis 在 publish 时不可用 → 档案隐藏到重启 | publish 重试 3 次；启动收敛；Redis 不可用时搜索本身也不可用，用户不会看到「隐藏但可用」的矛盾态 |
| 队列在启动收敛前恢复，重新标记尚未完成的档案 | 收敛在队列启动之前调用；即使竞态，重新标记的也是真正未完成的档案 |
| 大库 `SCAN` 成本 | pending 键极少；`SCAN COUNT 500` 循环，量级与一次搜索相当；engine 侧已有 `KEYS`，本 feature 不放大 |
| 现有测试依赖「记录写入即可搜」 | 只在新增测试里构造 pending；现有测试不写该键，默认行为不变 |

## Complexity Tracking

无宪法违规需要豁免。唯一需要记录的行为变化：pending 期间 `GET /api/archives/{id}/metadata|files|page` 由「可能 200」变为 404——这是本 feature 的目的，且这些档案在完成前从未对客户端构成稳定契约。
