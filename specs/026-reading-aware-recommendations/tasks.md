# Tasks — 阅读状态感知的推荐排序

**Feature**: 026-reading-aware-recommendations | **Input**: spec.md, plan.md, research.md, data-model.md, contracts/, quickstart.md

**Prerequisites**: plan.md ✅、research.md ✅、data-model.md ✅、contracts/ ✅、quickstart.md ✅

## Phase 1: Setup

- [x] T001 确认实现落点：`crates/lanrurugi-recommend/src/{lib.rs,recommend.rs}`、`crates/lanrurugi-api/src/{recommend.rs,recommend_llm.rs,recommend_precompute.rs,llm_prompts.rs}`、`crates/lanrurugi-storage/src/keys.rs`，记录各文件现有签名（见 plan.md「Project Structure」）。

## Phase 2: Foundational — 纯函数层

- [x] T002 [P] 新增 `crates/lanrurugi-recommend/src/reading.rs`：`ReadingState`、`ReadBucket`、`FINISHED_PROGRESS = 0.80`、`state_boost(bucket)`、`series_evidence(anchor, cand)`（artist/circle/parody 交集或 `normalize_title` 前 24 字符）、`rank(anchor, anchor_state, candidates, limit, include_finished) -> RankOutcome`（过滤 Finished、`final = similarity + min(boost, 0.15)`、降序、同分 id 升序、全读完兜底前 3）、`progress_digest(anchor_bucket, ordered)`（16 位十六进制）。
- [x] T003 [P] 在 `crates/lanrurugi-recommend/src/lib.rs` 导出 `reading` 模块；`recommend.rs` 的既有 `recommend()` 保持签名不变（避免影响其它调用点），内部可选复用 `rank()` 的排序键。
- [x] T004 [P] `reading.rs` 单测：分档边界（0、1.0、0.80、`lastreadpage == pagecount`、`pagecount == 0`）、加成值、同系列证据（标签/标题前缀/门槛 0.72）、排序稳定（同分按 id）、兜底前 3、digest 稳定且对顺序敏感。

## Phase 3: US1 — 已读完不再推荐（缓存命中与未命中两条路径）

- [x] T005 在 `crates/lanrurugi-api/src/recommend.rs` 新增私有 helper：`fn reading_state(a: &Archive) -> ReadingState`；`recommendations()` 的未命中路径在余弦打分后构造 `Vec<RankedCandidate>` 并调 `rank()`。
- [x] T006 `recommendations_from_cache()`：读 Top-N 后逐 id `archives.get` 时同时取 `lastreadpage/pagecount/lastreadtime`，构造 `RankedCandidate` 后走同一个 `rank()`；`rehydrate_tags_and_pagecount()` 改为同时补阅读字段或直接被替代。
- [x] T007 `get_recommendations`：`TANK_` 分支保持「锚点=最后一个成员、排除全部成员」，锚点的 `ReadingState` 也取自该成员。
- [x] T008 [P] `crates/lanrurugi-api/src/recommend_llm.rs` 的 `PREFILTER_COUNT` 100 → 150（它同时是读路径 prefilter 和 `recommend_precompute` 的 Top-N 下限，不需要新常量）。

## Phase 4: US2 — LLM 只看未读完并带进度

- [x] T009 `crates/lanrurugi-api/src/recommend_llm.rs`：进入 prompt 的候选先剔除 `Finished`；shortlist 元素携带 `ReadBucket` 文案（未读/在读 x%/进度未知）；锚点带「刚读完 或 在读 x%」。
- [x] T010 `crates/lanrurugi-api/src/llm_prompts.rs`：`RECOMMEND_RERANK_PROMPT` 升到 v2，明确「优先同系列下一卷、已读完的不要选、最多 10 条」；`RECOMMEND_RERANK_PROMPT_VERSION` 同步 `"v2"`。
- [x] T011 `llm_rerank` 的结果校验：丢弃不在候选集、重复、`Finished` 的 id；不足 `limit` 时按 `rank()` 的顺位补足；`cached_picks`/`remember_picks` 的键加 `progress_digest`，`RerankCacheEntry` 增 `progress_digest` 并双重校验。
- [x] T012 `crates/lanrurugi-storage/src/keys.rs`：`recommend_rerank_key(archive_id, limit, prompt_version, progress_digest)`，键拼接 digest 段；更新所有调用点。

## Phase 5: US3 — 同系列卷序

- [x] T013 ~~校验同系列加成在两条路径都生效~~ — 不做。这条规则完全在 `reading::rank`（纯函数）里，`finished_candidates_are_dropped_and_series_unread_wins` 已用 0.99 已读完 / 0.96 同系列未读 / 0.88 无关高分断言了完全相同的场景；API 层只会把同一批候选喂给同一个函数，再断言一遍是重复。2026-11-12 决定，用户明确不希望堆积无用的测试。

## Phase 6: US4 — 无 LLM / 缓存一致性

- [x] T014 ~~[P] API 测试：缓存命中与未命中顺序一致 / 已读完被过滤 / 全读完返回 3 条~~ — 不做，理由写在这里免得以后又被当成漏项：
  - 三条断言里的「已读完被过滤」「全读完返回 3 条」「同分稳定」已经由 `reading.rs` 的单测覆盖；
  - 「缓存命中 vs 未命中顺序一致」在 API 层无法构造：`recommendations_from_cache` 也要先 `self.embedder.embed(anchor)`，而 `test_state` 的 `RecommendService` 指向空的临时模型目录，两条路径都会在 ONNX 那一步失败。要让它可测就得给锚点向量加一个测试缝——那是新机制，不是测试。
  - 顺序不依赖输入顺序这一点改在纯函数层验证（`the_order_does_not_depend_on_the_input_order`），这正是两条路径唯一可能分歧的地方。
  - 接线本身（缓存路径从档案行构造 `ReadingState`、把锚点进度传给 LLM）在 dev 上做过实测：三个锚点各取 6 条，18 条全是 `is_read: false`，同系列未读卷排在最前。
- [x] T015 [P] prompt 快照/单测：`llm_prompts` 的 v2 文案包含进度字段且不含旧 v1 的「只按相似度」措辞。

## Phase 7: Polish

- [x] T016 `mise run fmt-check`、`mise run clippy`（workspace）、`mise run test-crate -- lanrurugi-recommend`、`mise run test-crate -- lanrurugi-api` 全绿。
- [ ] T017 `mise run dev-rebuild && dev-down && dev-up` 后按 quickstart.md 场景 1–5 手测；记录实际顺序与 Redis 键。（2026-11-12 只做了接口级抽查：dev 镜像已含 `9e19c25`，三个锚点各取 6 条全部 `is_read: false`；五个场景未逐条走。）
- [x] T018 更新 `specs/026-reading-aware-recommendations/tasks.md` 勾选、`docs/` 相关说明（已更新 `docs/comparison-with-lanraragi.md` 推荐引擎一条）；提交为 `9e19c25`，PR #258 正文已加第 19 节。

## Dependencies

- T002 → T003 → T004（纯函数层先于 API 接入）
- T005/T006 → T007 → T013/T014
- T008 独立（只影响预计算数量）
- T009/T010 → T011 → T012
- T016 → T017 → T018

## Parallel

- T004 与 T008 可并行；T014 与 T015 可并行；T005 与 T009 可并行（不同文件）。


## Implementation Record

**2026-11-11** — 全部代码任务（T001–T016）已完成：

- `crates/lanrurugi-recommend/src/reading.rs`：`ReadingState`/`ReadBucket`/`rank()`/`series_evidence()`/`progress_digest()`；8 个单测覆盖分档边界、加成、同系列证据、同分稳定、全读完兜底、digest。
- `crates/lanrurugi-api/src/recommend.rs`：未命中路径（全量向量评分）与命中路径（Top-N + 向量）都构造 `RankedCandidate`（含 `lastreadpage/pagecount/lastreadtime`），统一走 `rank()`；两条路径的 LLM 调用都拿 `outcome.order` 重排后的候选；全读完时 `tracing::debug!` 记录兜底。
- `crates/lanrurugi-api/src/recommend_llm.rs`：`PREFILTER_COUNT` 100→150；`llm_rerank` 改为接收 `&[RankedCandidate]` + 锚点 `ReadingState`，prompt 每条带「阅读进度」、锚点带「当前阅读进度」；短名单仍按作品去重并截断 40；结果校验/补足沿用 `rank_picks`；重排缓存键 = `archive:limit:prompt_version:progress_digest`，条目里也存 digest。
- `crates/lanrurugi-api/src/llm_prompts.rs`：`RECOMMEND_RERANK_PROMPT_VERSION` → `v2`，新增阅读进度规则（未读 > 在读 > 进度未知；已读完不在名单里）。
- `crates/lanrurugi-storage/src/keys.rs`：`recommend_rerank_key(archive_id, limit, prompt_version, progress_digest)`。

**检查**：`mise run fmt-check` ✅；`mise run clippy`（workspace，`-D warnings`）✅；`mise run test-crate -- lanrurugi-recommend` 17 passed ✅；`mise run test-crate -- lanrurugi-api` 668 passed ✅。

**T017 dev 实测（2026-11-11，新二进制，`version_name` 仍是 HEAD 的 `1b0752c`）**：

- 锚点 `f3cbbcd8…`（p=120/125）：修复前旧容器返回 10 条里含 `Vinnegal Albedo`（71/71，已读完）；新容器返回 10 条全部 `is_read=false`，无已读完作品。
- 缓存一致性：同一锚点连续两次请求，id 与分数完全一致（第二次命中 rerank 缓存）。
- 进度影响：把候选 `3d064e02…` 的 Redis 字段改成 `progress=845,lastreadtime=…`（Finished），该 id 立即从结果消失；恢复 `progress=0,lastreadtime=0` 后重新出现。
- 缓存键：`LANRURUGI_RECOMMEND_RERANK:<id>:10:v2:<digest>`；上一步前后 digest 从 `e6a646baa74354ba` 变为 `7f672bb245f86ef9`，确认进度摘要进键。
- 全读完兜底 3 条由 `reading.rs` 单测覆盖，未在真实书库上把全部候选标成读完。

**已知未做**：T013 的 API 层顺序断言、T014 的「缓存命中/未命中顺序一致」集成测试都未写——前者由 `reading.rs` 单测覆盖，后者由两条路径共享 `rank()` 的结构保证（`recommend.rs` 原本没有测试模块，补 AppState/Redis 夹具的成本高于本特性的收益）。
