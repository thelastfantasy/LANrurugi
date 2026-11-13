# Implementation Plan: 入库完整性

**Branch**: `feat/share-bind-interface` | **Date**: 2026-11-13 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/027-ingest-integrity/spec.md`

## Summary

把「下载 → 暂存 → 书库」整条链上会丢数据或放过坏文件的环节补齐：复制核对长度并在删源前持久化（G1/G2），改名前后保证落盘顺序（G3），续传只从已确认落盘的位置继续（G4），每个新档案带全文件 BLAKE3 并在跨文件系统复制时比对（G5），失败一律保留源字节（G6）。另提供默认关闭的流水线回读「严格校验」，以及指纹维护与存量补算。

P1（M1–M3）防丢数据，计划进 beta6；P2/P3（M4–M7）可以下一版。

## Technical Context

**Language/Version**: Rust（workspace edition 与现有一致），TypeScript（设置页、i18n）

**Primary Dependencies**: `tokio`、`blake3 1.8.7`（已有）、`libc 0.2`（已有，macOS `F_FULLFSYNC`/`F_NOCACHE`、Linux `O_DIRECT`、`statfs`）

**Storage**: Redis（档案 hash 新字段 `blake3`；队列项 JSON 新字段 `staged_blake3`；`LRR_CONFIG` 新布尔 `ingest_strict_verify`），磁盘上 `.meta.json` 新字段 `confirmed_len`

**Testing**: `mise run test-crate -- lanrurugi-core lanrurugi-api lanrurugi-scanner lanrurugi-storage`（Redis 依赖的用 `LANRURUGI_TEST_REDIS_URL`），vitest（设置页开关），quickstart 场景 A–G

**Target Platform**: Linux（容器与原生）、macOS arm64、FreeBSD x86_64（015）

**Project Type**: web-service（单二进制 + React 前端）

**Performance Goals**: 严格校验关闭时入库耗时增幅 ≤ 5%（SC-003）

**Constraints**: 严格校验的内存上限 = 同时复制数 × 3 块 × 块大小（≤ 4 MiB）；补算任务遵守 rayon ~30% 核心上限与 `LoadThrottle`

**Scale/Scope**: 单文件可达数 GB；存量书库约 1.1 TB（补算任务）

## Constitution Check

| 原则 | 结论 |
|---|---|
| I. 数据兼容 | 通过：档案 id 算法不变；所有新字段可选，旧数据按「无指纹」处理；不做破坏性迁移，补算由用户手动触发 |
| II. API 契约 | 通过：`/api/archives/{id}/metadata` 只增可选字段 `blake3`；docgen sidecar 同步 |
| III. 资源与并发 | 通过：BLAKE3 符合「新哈希优先 BLAKE3」；复制路径不新增额外整文件读取；严格校验默认关闭且有界；补算任务批量走受限 rayon 池而非逐个 `spawn_blocking` |
| IV. 插件沙箱 | 不涉及（插件契约不变；`file_path` 产物在接手时计算指纹） |
| V. 机密与网络边界 | 不涉及 |
| VI. 分期 | 通过：P1 与 P2/P3 可独立交付 |
| VII. 前端 | 设置开关与错误文案走现有组件与 14 个 locale；纯视觉部分交由用户验收 |

## Project Structure

### Documentation (this feature)

```text
specs/027-ingest-integrity/
├── spec.md
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/io-contract.md
├── checklists/requirements.md
└── tasks.md              # /speckit-tasks 生成
```

### Source Code (repository root)

```text
crates/lanrurugi-core/src/io.rs                      # copy_file / durable_rename / sync_dir / 严格校验
crates/lanrurugi-core/src/queue_error.rs             # 四个新错误变体
crates/lanrurugi-core/src/entities.rs                # Archive.blake3
crates/lanrurugi-storage/src/download_queue.rs       # staged_blake3、PendingFilenameConflict.blake3
crates/lanrurugi-storage/src/repository.rs           # 只 HSET blake3 的写入口
crates/lanrurugi-api/src/download_manager/stream.rs  # 写入时哈希、确认点、续传截断
crates/lanrurugi-api/src/download_manager/bundle.rs  # 打包输出流哈希
crates/lanrurugi-api/src/download_manager/mod.rs     # rename_or_copy 重写为显式顺序
crates/lanrurugi-api/src/download_manager/ingest.rs  # 基准比对、失败处理、记录写入 blake3
crates/lanrurugi-api/src/upload.rs                   # 接收时哈希
crates/lanrurugi-api/src/share_net/import.rs         # 复用 iroh 指纹
crates/lanrurugi-api/src/settings.rs                 # ingest_strict_verify
crates/lanrurugi-api/src/archive_split.rs 等改写字节的操作  # FR-017
crates/lanrurugi-api/src/integrity_backfill.rs       # 新：补算任务
crates/lanrurugi-scanner/src/relocate.rs             # 改用新 copy_file
apps/frontend/src/pages/Settings/...                 # 开关、补算入口
apps/frontend/src/i18n/locales/*.json                # 错误与设置文案
docs/api/sidecar/                                    # metadata.blake3
```

## Phase 0: Research (summary)

见 [research.md](./research.md)：R1 不用 `blit-core`；R2 BLAKE3；R3 在首次经手时增量计算，续传时重读已确认前缀重建 hasher；R4 持久化顺序与 macOS `F_FULLFSYNC`；R5 确认点 64 MiB/10 s；R6 严格校验用有界队列 + `O_DIRECT`/`F_NOCACHE`，ZFS 上判定为无法验证并跳过；R7 不一致时的处理；R8 补算任务。

## Phase 1: Design

见 [data-model.md](./data-model.md)、[contracts/io-contract.md](./contracts/io-contract.md)、[quickstart.md](./quickstart.md)。

## Implementation Breakdown

### M1 — 复制与持久化（G1、G2、G6；US1，P1）

- `io::copy_file` 返回 `CopyOutcome`：每块 `blake3::Hasher::update`；结束核对 `copied == len` 与 `dest.metadata().len() == len`；`sync_all(dest)`；失败删目标、返回 `CopyError`。
- 新增 `sync_dir`、`durable_rename`；macOS 分支用 `F_FULLFSYNC`，Linux/FreeBSD 普通 `fsync`；非 Linux 分支在 Linux 上也编译并有单测（CLAUDE.md 015 规则）。
- 删除 `move_file`；`rename_or_copy` 改为：同文件系统 → `durable_rename`；跨文件系统 → `copy_file` →（M4 之后：比对基准）→ 改名进桶 → `sync_dir` → 删源。
- `relocate.rs:393` 改用新 `copy_file`。
- 单测：短读、写入/同步失败、长度不符，源均保留（quickstart A）。

### M2 — 改名顺序（G3；US1，P1）

- `ingest.rs:365/390`、`stream.rs:706` 的裸 `rename` 改为 `durable_rename`。
- `.ingest-*` 改名进桶后 `sync_dir(bucket)`，然后才写记录、删源（data-model 的顺序图）。
- quickstart B。

### M3 — 续传确认点（G4；US2，P1）

- `ResumeMetadata.confirmed_len`；下载循环按 64 MiB/10 s `sync_data` 并原子更新 `.meta.json`。
- 续传：有 `confirmed_len` → `set_len` 截断后以其为起点；无 → 删除重下。
- 与 `If-Range` 的关系：只在 `If-Range` 判定同一内容时才用确认点；否则照旧丢弃。
- quickstart C；测试服务器复用 `stream.rs` 现有测试里的本地服务器。

### M4 — 基准指纹与比对（G5；US3，P2）

- `stream.rs` 写循环增量哈希；续传时重读 `[0, confirmed_len)` 重建 hasher。
- `bundle.rs` 输出流包 hashing writer；`upload.rs` 接收时哈希（含分片上传：按块顺序 `update`，乱序/重传块以最终组装文件为准，在 `complete` 时读一遍）。
- 插件 `file_path`、启动接管、旧队列收编：接手时读一遍。
- 共享导入：沿用 iroh 已校验的文件哈希。
- `DownloadQueueItem.staged_blake3` / `PendingFilenameConflict.blake3` 持久化；`catalogue_staged_file` 增加 `expected_blake3: Option<Hash>` 参数；跨文件系统时与 `CopyOutcome.blake3` 比对，同文件系统直接沿用。
- 记录写入 `Archive.blake3`；不一致按 R7 处理（`StagedContentCorrupted`，下载来源进入需要重新下载，接管来源只报错）。
- 监视器直接发现的书库文件：扫描入库时读一遍算指纹（与缩略图生成同批，批量走受限 rayon）。
- quickstart D。

### M5 — 严格校验（US4，P3）

- `CopyOptions.strict_verify`；`mpsc` 容量 3；校验任务 `spawn_blocking` + 独立 fd（Linux/FreeBSD `O_DIRECT` 对齐缓冲区；macOS `F_NOCACHE`）。
- `statfs` 识别 ZFS 或 `O_DIRECT` 打开失败 → 记录「无法验证磁盘」并跳过（FR-016）。
- 设置项与设置页开关（含说明文案）。
- quickstart E；在 NAS 上实测并记录。

### M6 — 指纹维护与补算（US5，P3）

- 分卷拆分、补页、覆盖式重复处理等改写字节的路径完成后更新 `blake3`（逐个 grep 写入书库文件的调用点，列入 tasks.md）。
- `integrity_backfill.rs`：后台任务，受限 rayon + `LoadThrottle`，只 `HSET blake3`，可取消；设置页维护区入口。
- `/api/archives/{id}/metadata` 增加 `blake3`；docgen sidecar；`mise run docs-api -- --check`。

### M7 — 测试、文档与验收

- 全部 quickstart 场景；`mise run check`；`mise run test`（detached）。
- `README.md`「能做什么」与 `docs/comparison-with-lanraragi.md` 视情况补一条「入库完整性」。
- tasks.md 写 Implementation Record：哪些在真机上验证、哪些只在单测里验证。

## Risks & Mitigations

| 风险 | 缓解 |
|---|---|
| fsync 在某些 NAS 文件系统上很慢 | SC-003 实测；目录 fsync 每档案最多两次；确认点频率可调 |
| `O_DIRECT` 对齐错误导致 EINVAL | 读缓冲按 4 KiB 对齐分配；EINVAL 归为「无法验证」并跳过，不让入库失败 |
| 续传前缀重读增加续传开销 | 只在续传时发生；与「从头重下」相比仍省一半以上 |
| 分片上传的乱序块无法流式哈希 | `complete` 时读一遍组装文件，接受一次额外读取 |
| 新错误类型让旧前端显示原始代码 | 14 个 locale 同步补文案；未知变体回退通用错误文案 |
| 漏改某个改写字节的路径，指纹过期 | M6 先 grep 列全写入书库文件的调用点；补算任务可修复，以后的巡检 spec 会以指纹不一致报出 |

## Complexity Tracking

无违反宪法的复杂度需要说明。
