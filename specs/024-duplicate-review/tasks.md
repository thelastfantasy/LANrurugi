# Tasks: 重复档案检测与智能对比

**输入**：`spec.md`、`plan.md`、`research.md`、`data-model.md`、`contracts/http-api.md`、`quickstart.md`
**测试**：后端以 `#[cfg(test)]` 单测为主；前端以 tsc/eslint + 手工验收为主（仓库无组件测试基建）。

## Phase 1 — 基础（哈希与键）

- [x] T001 在 `crates/lanrurugi-api/src/duplicates/keys.rs` 定义四个键名与 group key 规则（成员 id 升序、逗号连接），并写单测。
- [x] T002 `crates/lanrurugi-api/src/duplicates/hash.rs`：读缩略图 → `imgcompare::alignment::hash_page` → 写 `LANRURUGI_DUP_COVER_HASH`（`v1:<hex>`），命中即复用；写单测（同一张图两种编码得到同一哈希附近、缺失缩略图返回 `None`）。
- [x] T003 `Cargo.toml` 给 `lanrurugi-api` 补 `lanrurugi-imgcompare`（若尚未依赖）与 `image_hasher` 的直接依赖（如需要）。

## Phase 2 — 检测（US1 / US5）

- [x] T004 `duplicates/detect.rs`：8×8 bit 鸽笼分桶，生成候选对（汉明距离 ≤ threshold）。
- [x] T005 `duplicates/detect.rs`：元数据证据（`release_markers::clean_title` 后三元组 Jaccard、作者/社团标签、页数差、大小比），生成 `GroupRecord.pairs`。
- [x] T006 `duplicates/detect.rs`：并查集成组、计算 `chained`/`middle`，写 `LANRURUGI_DUP_GROUPS`，同时写 legacy `LRR_DUPLICATE_GROUPS`。
- [x] T007 重写 `duplicates.rs` 的扫描端点：作业化（`jobs`）、分阶段进度、取消检查点、409 并发保护。
- [x] T008 `GET /database/duplicates/groups` 与 `/{key}`：读取组、合并决定与复核缓存；空态形状。
- [x] T009 单元测试：分桶召回（构造 3 组近邻 + 1 个远点）、元数据否决（不同作者）、链式组标记、legacy 键写入。

## Phase 3 — 对比（US2）

- [x] T010 `duplicates/compare.rs`：`spawn_blocking` 调 `imgcompare::compare_archives`，裁剪成 `data-model.md` 的对比形状（samples 限 12、`favor` 逐对判定、`a_only/b_only`）。
- [x] T011 `GET /database/duplicates/compare`：404（档案不存在）、409（入库中）、503（线程池满 + `Retry-After`）。
- [x] T012 专用 `rayon::ThreadPool`（`(cores*3/10).clamp(1,4)`），扫描与对比共用。
- [x] T013 单测：两个构造档案的 CompareResult 形状（对齐数、缺页列表、语言判定透传）。

## Phase 4 — LLM 复核（US4）

- [x] T014 `FeatureId::DUPLICATE_REVIEW` + 提示词（`llm_prompts.rs` 或同目录新文件），输入输出按 `contracts/http-api.md`。
- [x] T015 `duplicates/review.rs`：构造输入（成员 ≤ 8）、调用、校验、写 `LANRURUGI_DUP_REVIEW`、缓存命中跳过。
- [x] T016 校验失败/模型不可用 → 本地结论（页数完整 > 清晰度 > 大小）+ `fallback: true` + 本地 `reason`。
- [x] T017 单测：输出校验矩阵（枚举非法、index 缺失/重复、confidence 越界、reason 空）、缓存键稳定性、fallback 形状。

## Phase 5 — 决定（US3）

- [x] T018 `duplicates/decisions.rs`：读写 `LANRURUGI_DUP_DECISIONS`，默认全 `keep`，键校验（400/404）。
- [x] T019 `not_duplicate` 参与下一次扫描：被否决的对不进候选；因此组可能拆分。
- [x] T020 `POST/GET /database/duplicates/decisions` 与幂等性单测。

## Phase 6 — 前端（US1/US2/US3）

- [x] T021 `apps/frontend/src/api/types.ts` + `hooks.ts`：四个新接口的 hook 与类型。
- [x] T022 重写 `pages/Duplicates.tsx` 为 grid 卡片页（`GroupCard.tsx`）：组头、成员行（多选勾选、推荐角标、`+N`）、动作区（整宽「智能对比」、「保留勾选的 N 个」、「只保留推荐的」、溢出菜单）。
- [ ] T023 从删除前的提交恢复 `OverlayPage` / `MagnifierOverlay` / `AlignmentBandOverlay` / `PageItem` / `SourceItem`（`git log --diff-filter=D` 定位），改为接收新对比形状。
- [ ] T024 `SmartCompareModal.tsx`：顶部固定建议与两个按钮、并排同步翻页、放大镜、对齐带（点格跳页）、`N 本一览`。
- [x] T025 i18n：en / zh / zh_Hant / ja 四份补齐全部新键；颜色只进 5 个主题文件（如需新类）。
- [x] T026 空态、扫描中（进度条 + 取消）、错误态（档案被删、入库中）。

## Phase 7 — 文档、验收与门禁

- [x] T027 `docs/src/content/docs/guide/duplicates.md`（中英）：检测原理、阈值含义、LLM 角色与费用、删除的不可逆性、`not_duplicate` 的语义。
- [ ] T028 跑 `quickstart.md` 全部步骤，记录结果（含一次 LLM 不可用的 fallback 验证）。
- [x] T029 门禁：`check-crate`、`clippy`、`fmt-check`、`test-crate -- lanrurugi-api`、`tsc -b --force`、`eslint src`；`mise run docs-build`。
- [x] T030 写 `specs/review-report-2026-11-10-duplicates.md` 供 cc review（实现摘要、偏离、验证证据、未做项）。

## 依赖关系

- T001 → T002/T004；T004 → T005 → T006 → T007；T006 → T008；T010 → T011；T014 → T015 → T016；
  T018 → T019；后端 T008/T011/T020 就绪后前端 T021 → T022 → T024；T023 可并行。
- 可并行：T003、T010（与检测解耦）、T021。

## MVP

T001–T011 + T018–T024 + T029：扫描、卡片、对比、决定、删除。LLM（T014–T017）与文档可后置，
但 spec 把 LLM 列为 P2 可降级路径，所以 UI 必须从一开始就能在 `fallback` 下工作。


## Implementation Record

**Done**: T001–T022, T025–T027, T029, T030. The scan, grouping, review, decisions, compare endpoint,
frontend card page and the docs page are on `feat/share-bind-interface`.

**Partial**

- **T023/T024** — the comparison modal was rebuilt from scratch rather than restoring the deleted
  `OverlayPage`/`MagnifierOverlay`/`AlignmentBandOverlay` components. It shows the sampled pairs
  side by side with sharpness, the aligned count and the one-sided pages, but has no magnifier and
  no click-an-aligned-cell-to-jump behaviour. Pages come from the per-page thumbnail endpoint
  (`/api/archives/{id}/thumbnail?page=N`), not full-size page bytes, so it is a comparison aid rather
  than a pixel-inspection tool.
- **T028** — `quickstart.md` was not run: the dev instance needs a password this session did not
  have (same blocker as 020's T027), and the three intended fixture archives were not built.

**Deviations from plan**

1. `not_duplicate` is implemented by rebuild-ding components from the surviving pairs
   (`detect::regroup_by_pairs`) instead of filtering the candidate edge before grouping: the effect
   is the same (a rejected pair can never hold a group together) and a member whose only edge was
   rejected drops out instead of lingering as a singleton in a bigger group.
2. Grouping keys on cover hash + metadata, not on `thumbhash` (legacy's SHA-1 of cover bytes). The
   old value is still written to `LRR_DUPLICATE_GROUPS` unchanged for compatibility.
3. The reviewer is called once per group during the scan, and a fallback verdict is never cached —
   otherwise turning a model on later would never take effect.
4. `size_ratio < 0.2` and `pages_ratio < 0.8` rejections were added to the metadata gate beyond the
   plan's wording; without them a compressed 8 MB thumbnail collage and a 280 MB scan of the same
   cover still grouped.
