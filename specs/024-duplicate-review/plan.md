# Implementation Plan: 重复档案检测与智能对比

**Spec**: `specs/024-duplicate-review/spec.md`
**Branch**: `feat/share-bind-interface`（spec 明确要求沿用当前分支）

## Summary

把 `/duplicates` 从「封面 SHA-1 + 平铺表格 + 只有删除」换成「感知哈希候选 + 组卡片 + 智能对比
modal + 逐成员决定」。检测先宽后严：封面感知哈希 + 元数据圈候选（O(n) 建桶），只对候选跑
`lanrurugi-imgcompare` 的逐页对齐；LLM 只做「这组算不算同一作品、各自该留该删」的语义裁决，
只看文本与数字，失败即退回本地结论。任何自动化路径都不删除档案。

复用三块现成能力，不新造：

- `crates/lanrurugi-imgcompare`：`alignment::hash_page`（感知哈希）、`compare_archives`
  （逐页对齐 + 拉普拉斯清晰度 + 裁剪对齐 + 语言判定 + 建议）现在没有任何调用方，正好接回来；
- 016 单行本归组那套 `FeatureId` + `llm_prompts.rs` + review 缓存 + 标签沉淀；
- `/duplicates` 已有的扫描作业与 legacy 兼容键 `LRR_DUPLICATE_GROUPS`。

## Technical Context

- **语言/框架**：Rust（Axum/Tokio/Rayon），React 19 + TS + TanStack Query。
- **存储**：Redis 配置库（`DB 2`）新增四个键（见 data-model），legacy `LRR_DUPLICATE_GROUPS`
  继续写；不改档案主记录结构。
- **图像**：`image`/`image_hasher`（封面哈希）、`lanrurugi-imgcompare`（逐页比对）。
  CPU 密集部分走 `spawn_blocking`，Rayon 线程数按 `precompute_worker_budget` 同款公式压到
  可用核数的 30%。
- **LLM**：`lanrurugi_llm` 现有通道，`FeatureId::DUPLICATE_REVIEW`，提示词进 `llm_prompts.rs`。
- **前端**：页面 `apps/frontend/src/pages/Duplicates/`；对比 modal 的三个视觉组件从删除前的
  提交恢复（`git log --diff-filter=D --name-only` 定位），数据源改为新接口。
- **门禁**：`check-crate`、`clippy`、`fmt-check`、`test-crate`（api/scanner/imgcompare）、
  `tsc -b`、`eslint`；CI 为行为验证。

## Constitution Check

| 原则 | 结论 |
|---|---|
| I 数据完整性（不丢用户数据） | PASS：删除只作用于用户显式标为 `discard` 的成员，且走现有删除流程；检测与复核只写缓存与建议。 |
| II 兼容 legacy 契约 | PASS：`/database/duplicates` 的旧形状保留；`LRR_DUPLICATE_GROUPS` 继续写。新接口是 additive。 |
| III 不阻塞请求路径 | PASS：扫描是作业；比对走 `spawn_blocking`；LLM 复核在作业内串行、每组一次。 |
| IV 资源上限 | PASS：图像解码线程 ≤ 30% 核数；扫描作业带进度与取消。 |
| V 可观测 | PASS：作业进度、扫描汇总、复核缓存命中/退回都有日志与计数。 |
| VI 独立演进 | PASS：不改 020–023 的共享引擎；只读档案路径。 |
| VII 默认安全 | PASS：`not_duplicate` 与决定只影响本实例；LLM 不接收图像或档案字节。 |

## Project Structure（只列新增/主要改动）

```text
crates/lanrurugi-api/src/
  duplicates.rs                 # 现有 legacy 端点 + 新端点（扫描/组/对比/决定）
  duplicates/
    hash.rs                     # 封面感知哈希（读取 thumb、缓存、版本）
    detect.rs                   # 分桶、元数据证据、并查集成组
    compare.rs                  # 调用 imgcompare，裁剪成 API 形状
    review.rs                   # LLM 复核：构造输入、校验输出、缓存、退回
    decisions.rs                # 逐成员决定与 not_duplicate 的读写
crates/lanrurugi-core/src/
  entities.rs                   # （无改动）仅使用现有 Archive 字段
  queue_error.rs                # （无改动）
crates/lanrurugi-llm/src/
  prompts/duplicate_review.rs   # 提示词（或并入 llm_prompts.rs，按现有布局）
apps/frontend/src/pages/Duplicates/
  index.tsx                     # grid 卡片页
  GroupCard.tsx                 # 组头 / 成员行 / 动作区
  SmartCompareModal.tsx         # 恢复的对比 modal（并排翻页 + 放大镜 + 对齐带）
  components/…                  # 从删除前提交恢复的 OverlayPage / MagnifierOverlay / AlignmentBandOverlay
apps/frontend/src/api/
  hooks.ts                      # useDuplicateGroups / useDuplicateScan / useDuplicateCompare / useDuplicateDecisions
  types.ts                      # 新形状
```

## Phase 0（research.md）

R1 封面哈希；R2 分桶与阈值；R3 元数据证据与权重；R4 成组与链式组；R5 决定模型；
R6 LLM 复核契约与缓存；R7 比对接口形态（先同步、后流式）；R8 legacy 兼容；R9 UI 与组件恢复；
R10 CPU 上限与作业化。

## Phase 1（data-model.md / contracts/ / quickstart.md）

实体、Redis 键、JL 形状、状态机、HTTP 契约、LLM 契约、手工验收步骤。

## Phase 2（tasks.md）

按「哈希 → 检测 → 对比接口 → LLM → 决定 → 前端 → 文档与门禁」顺序，每步可独立验证。

## Complexity Tracking

| 决定 | 理由 |
|---|---|
| 新增四个 Redis 键而不是扩 `Archive` | 缓存与决定都是派生数据，不该进档案记录；清库重建时可丢。 |
| 封面哈希单独缓存 | 10 万档案重算一次要解码 10 万张封面；只在档案内容哈希变化时重算。 |
| 比对先同步返回 | `compare_archives` 已经产出完整结果；先测真实延迟，超过 1.5 s 再加 SSE 流。 |
| LLM 只给建议 | 删除是不可逆操作，模型判断可能有误；UI 上人永远点最后一下。 |
