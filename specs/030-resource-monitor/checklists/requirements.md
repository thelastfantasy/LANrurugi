# Specification Quality Checklist: 应用内资源监视

**Purpose**: 在进入 plan/tasks 前验证 spec 的完整性与可测性
**Created**: 2026-11-25
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] 无实现细节泄漏（技术选型写在 plan/research，不在 spec）
- [x] 聚焦用户价值与可观测结果
- [x] 面向非实现者也能读懂（含"为什么需要它"的三次事故背景）
- [x] 所有必填章节已完成

## Requirement Completeness

- [x] 无 `[NEEDS CLARIFICATION]` 残留
- [x] 需求可测且无歧义（每条 FR 都有可观察的验收方式）
- [x] 成功标准可度量（SC-001…SC-009 都有数字或可判定条件）
- [x] 每条 FR 都带 MUST 且能对应到至少一个用户故事或边界条件
- [x] 边界条件已覆盖（无 cgroup/PSI/GPU、非 Linux、权限不足、标签隐藏、多观察者、时钟回拨、快照过大、重启后计数器归零、采样中进程被替换、Redis 不可达、进程树超过 64 条）

## Feature Readiness

- [x] 每条 FR 都有对应的验收路径（`quickstart.md` 场景 1–11）
- [x] 用户故事按优先级排列且可独立交付（US1 无面板也能交付）
- [x] 无跨故事循环依赖（US1 → US2 → US3 → US4 → US5 单向）
- [x] 已记录与宪法的交互（Principle II/III/VI 见 spec 的"与宪法的一致性"）

## Notes

- FR-005 是一条"指标清单"式需求，刻意列全：漏一个指标就要再开一轮，而它的验收方式是"每个数字能对应到一类瓶颈"（SC-005）。
- GPU/温度明确放在"尽力而为"档（FR-006），避免在无 GPU 的部署上阻塞本特性。
- FR-009 顺带修一个既有缺陷（`diagnostics.json` 是空文件），因为它与本特性共用同一个写盘路径，分两次改会重复劳动。
- FR-018 把范围定成"整个应用"（进程树 + Redis 进程级指标 + 模型调用统计），但明确不做外部 exporter——理由写在 research R12：多一个常驻进程就违反"面板关掉时零占用"。
- FR-017 的 `(debug)` 标注只加在本项目构建出来的进程上；`deno` 是运行时拉的，不标。
