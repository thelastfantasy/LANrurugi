# Implementation Plan: 阅读状态感知的推荐排序

**Branch**: `feat/share-bind-interface` | **Date**: 2026-11-11 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/026-reading-aware-recommendations/spec.md`

## Summary

推荐排序目前只有 embedding 相似度：读完《夏妻3》后同系列的未读卷不会提前，已读完的卷也照样出现。本计划把阅读状态（`lastreadpage`/`pagecount`/`lastreadtime`）纳入排序：过滤已读完候选，给未读/在读/进度未知加成，同系列且相似度足够时再给一次加成；LLM 重排拿到进度信息并按卷序挑选，但结果仍被服务端校验、已读完一律丢弃。预计算缓存扩大到 150 条，请求路径做进度过滤，避免进度变化触发重新 embedding。推荐接口的响应形状不变。

## Technical Context

**Language/Version**: Rust 2021（workspace），TypeScript/React 19（前端，本次无改动）

**Primary Dependencies**: `lanrurugi-recommend`（embedding/相似度）、`lanrurugi-api`（推荐管线、LLM 重排）、`lanrurugi-storage`（Top-N 与重排缓存键）、`lanrurugi-llm`（模型调用 seam）

**Storage**: Redis（`recommend_cache` 的 Top-N 与向量、`LANRURUGI_RECOMMEND_RERANK:*`）；阅读进度本身已在 `Archive` 实体里，不新增字段

**Testing**: `mise run test-crate -- lanrurugi-recommend`、`mise run test-crate -- lanrurugi-api`、`mise run clippy`、`mise run fmt-check`；dev 容器手测

**Target Platform**: Linux server（Docker 镜像 / 本机 dev 容器）

**Project Type**: Rust workspace + React 前端（单进程服务）

**Performance Goals**: 推荐接口的缓存命中路径不新增 O(n) 读取；进度过滤与加成在 150 条候选上完成，单次 <1ms

**Constraints**: 不引入卷号解析；不改推荐接口的响应形状；LLM 失败必须退回同样带阅读状态的顺序；Top-N 缓存不因进度变化重建

**Scale/Scope**: 单用户、书库上限按 10 万档案设计；本次改动集中在 `lanrurugi-recommend`、`lanrurugi-api` 的推荐管线与 `lanrurugi-storage` 的一个缓存键

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

- **I. 数据与 API 兼容**: 不新增持久化字段，不改 `/api/reader/recommendations/{id}` 的响应形状；旧 Redis 的 Top-N/重排缓存条目在键或指纹变化后自然失效，不需要迁移。✅
- **II. 文档化例外**: 无删除/改名既有 API 字段。✅
- **III. 资源意识、真并发**: 排序是 150 条候选上的纯 CPU 工作，不进入 `spawn_blocking` 循环；不在 async reactor 上做重活。✅
- **IV/V**: 无新增模型调用点；LLM 调用继续走 `lanrurugi-llm` 的 `CallSpec`（`FeatureId::RECOMMEND_RERANK`）。✅
- **VI. 单一可部署单元**: 改动落在既有 crate，不新增服务。✅
- **VII. 前端纪律**: 本次无前端改动；接口响应形状不变。✅

Post-design re-check: 设计未引入新的原则冲突（见 Phase 1 末尾）。

## Project Structure

### Documentation (this feature)

```text
specs/026-reading-aware-recommendations/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/reader-recommendations.md
├── checklists/requirements.md
└── tasks.md
```

### Source Code (repository root)

```text
crates/lanrurugi-recommend/src/
├── reading.rs          # 新增：进度分档、加成、同系列证据、排序（纯函数）
├── recommend.rs        # 保持纯相似度入口；复用 reading 的排序
└── lib.rs              # 导出 reading

crates/lanrurugi-api/src/
├── recommend.rs        # 两条路径接入进度过滤/加成；LLM 只拿未读完候选
├── recommend_llm.rs    # prompt 带进度；返回 id 校验；重排缓存键带进度摘要
├── recommend_precompute.rs  # Top-N 下限 150
├── llm_prompts.rs      # prompt v2
└── recommend_reading.rs # 新增（如需）：从 Archive 构造阅读状态的薄封装

crates/lanrurugi-storage/src/keys.rs  # recommend_rerank_key 增加 progress digest

apps/frontend/          # 无改动
```

## Phase 0: Outline & Research

见 [research.md](./research.md)。要解决的未知点：

1. 阅读状态放在哪一层（crate 纯函数 vs API 内联）——选 `lanrurugi-recommend`，可单测、无 Redis 依赖。
2. 两条路径（缓存命中/未命中）如何保证同一顺序——都先得到 `(ArchiveMeta, ReadingState, similarity)` 再走同一个排序函数。
3. 重排缓存如何随进度失效——键里加进度摘要（锚点档位 + 候选档位序列的短哈希）。
4. Top-N 150 的存储代价——只存 id 列表，向量本来就按档案存一份，代价可忽略。
5. 已读完过滤与「全库已读完」兜底——排序函数返回 `(items, fallback_used)`。

## Phase 1: Design & Contracts

- [data-model.md](./data-model.md)：`ReadingState`/`ReadBucket`/`RankedCandidate`/`ProgressDigest`/`RerankCacheEntry`。
- [contracts/reader-recommendations.md](./contracts/reader-recommendations.md)：接口响应形状不变，排序语义变化。
- [quickstart.md](./quickstart.md)：单测、crate 测试、dev 手测步骤。

### 关键设计决定

1. **阅读状态是派生值**：`ReadingState { last_read_page, page_count, last_read_time }` 由 `Archive` 的三个字段构造，不落库。
2. **分档**：
   - `Unread`：`last_read_time == 0 && last_read_page == 0`
   - `Finished`：`page_count > 0 && last_read_page >= page_count && progress >= 0.80`
   - `Unknown`：`page_count == 0 && last_read_page > 0`
   - `Reading(p)`：其余（`p` 夹在 0..1）
3. **加成**：`Unread +0.12`、`Reading +0.06 × (1 − p)`、`Unknown +0.03`，总加成上限 `0.15`（当前各档互斥，上限用于同系列叠加后的总封顶）。
4. **同系列证据**：两侧标签按 `namespace:value` 拆分，交集含 `artist`/`circle`/`parody` 任一即成立；或 `normalize_title` 后的标题前缀相同（取前 24 个字符比较，避免长标题尾部噪声）。仅当 `similarity >= 0.72` 时给 `+0.08`。
5. **排序**：`final = similarity + boost`，降序；同分按 id 升序，保证两条路径一致。
6. **LLM**：进入 prompt 的候选先剔除 `Finished`；prompt 每条带进度文案，锚点带「刚读完」；返回 id 若在 `Finished` 集合或不在候选集，丢弃并按 `final` 顺位补足。
7. **缓存**：`PREFILTER_COUNT` 100 → 150；重排缓存键 = `archive_id:limit:prompt_version:progress_digest`，`progress_digest` 由锚点档位 + 候选 id 的档位序列（按 final 顺序）取 `DefaultHasher` 的 16 位十六进制。
8. **兜底**：过滤后为空 → 按相似度取前 3 条（不带加成语义），排序函数返回 `fallback_used = true`；接口响应加一个可选的 `"fallback": true` 字段？**不加**——响应形状保持不变，前端标题本身是中性的「接下来读什么」。

### Post-design Constitution Check

- 不新增依赖、不新增服务、不新增持久化字段；重排缓存键变化只造成一次性冷启动。✅

## Complexity Tracking

无违反 Constitution 的设计；不需要例外记录。
