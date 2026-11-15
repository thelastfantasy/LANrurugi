# Tasks: 入库完整性（下载 → 暂存 → 书库）

**Input**: `specs/027-ingest-integrity/` 下的 spec.md、plan.md、research.md、data-model.md、contracts/io-contract.md、quickstart.md

**Tests**: spec 的每个 User Story 都写了 Independent Test，SC-001/002/005 要求故障注入测试，所以各故事都带测试任务。测试只跑改动到的 crate（`mise run test-crate -- <crate>`）；全量 `mise run test` 交给 GitHub CI，本地不跑。

**analyze（2026-11-14）后补入**：C1、U1、U2、U3、G1 已并入 T005、T012、T024、T026、T033。

**生成时对照代码（`feat/share-bind-interface` @ `938b456`）修正的计划细节**：

- 入库基准指纹放在 `StagedDownload.blake3`（`lanrurugi-storage/src/download_queue.rs:135`），不在队列行上另加 `staged_blake3`。原因：下载完成后在同一进程里立即入库；能跨重启保留下来的「已下载未入库」字节，只有入库失败时 `note_ingest_failure` 记下的 `StagedDownload` 这一处。`PendingFilenameConflict` 是 020 之前的遗留数据，不会再新建，接管时由复制现算，不加字段。
- 「启动时接管」现只剩 `adopt_pending_conflict`（`ingest.rs:118`），spec 引用的 `ingest.rs:888-1076` 已不存在。
- 插件 `file_path` 旧契约在当前代码里已无入库路径（`plugins.rs` 只把 `file_path` 作为插件参数传出），不单独做。
- 书库里没有「原地改写档案字节」的操作：补页写的是 `.patch.zip` 旁路文件；分卷拆分产出新档案并删除原档案；`ensure_repack` 只写缓存。所以 FR-017 落到「每次建立 / 替换档案记录都写入指纹」，由 scanner 管线统一负责（T024）：调用方给了指纹就用，没给就在管线里读一遍算。

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Setup

- [x] T001 确认 `blake3` 与 `libc` 已是 `lanrurugi-core` 的依赖；缺哪个就在 `crates/lanrurugi-core/Cargo.toml` 里以 `workspace = true` 加上（工作区根 `Cargo.toml` 已有两者）。

## Phase 2: Foundational（各故事共用）

- [x] T002 在 `crates/lanrurugi-core/src/queue_error.rs` 加四个变体，并给出 `code()` 编号 1014–1017：
  - `CopyLengthMismatch { path, expected, copied, dest_len }`
  - `DurabilityFailed { path, os_error }`
  - `StagedContentCorrupted { path, expected_blake3, actual_blake3 }`
  - `VerifyReadbackMismatch { path, offset, len }`

  `kind` 用 snake_case，与现有变体同样序列化。
- [x] T003 [P] 在 `apps/frontend/src/api/types.ts` 的 QueueError 联合类型里加上这四个 `kind`；在 `apps/frontend/src/components/Layout/QueueErrorText.tsx` 里各给一个分支；在 en / zh / zh_Hant / ja 四个 locale 里加文案。文案要说明下一步怎么做：`StagedContentCorrupted` 写「暂存文件已损坏，请重新下载」，接管来源写「原文件已保留」。
- [x] T004 在 `crates/lanrurugi-core/src/io.rs` 里新增：
  - `sync_dir(dir)`：打开目录后 `sync_all`。
  - `sync_file(file)`：macOS 走 `fcntl(F_FULLFSYNC)`，失败时退回 `sync_all`；其他平台直接 `sync_all`。macOS 分支抽成纯函数，Linux 上也编译并有单测（CLAUDE.md 015 规则）。
  - `durable_rename(from, to)`：先打开 `from` 做 `sync_file`，再 `rename`，最后对 `to` 的父目录 `sync_dir`。

## Phase 3: User Story 1 — 移动档案时绝不丢失唯一的一份（P1）🎯 MVP

**Goal**: 复制核对长度、删源前落盘，改名前后保证顺序，任何失败都保留源字节（G1/G2/G3/G6）。

**Independent Test**: quickstart A、B。三种情况都不删源，书库也不留下被记成档案的残缺文件：复制中源被截短、写入或同步失败、删源前中断。

- [x] T005 [US1] 按 contracts/io-contract.md 重写 `crates/lanrurugi-core/src/io.rs` 的 `copy_file(from, to, &CopyOptions) -> Result<CopyOutcome, CopyError>`：
  - 每块都 `blake3::Hasher::update`。
  - 结束时核对 `copied == len`，以及 `dest.metadata().len() == len`。
  - 然后 `sync_file(dest)`。
  - 结束后再取一次源的 `metadata().len()`，与开始时的 `len` 不同也判 `LengthMismatch`（源在复制中被追加或替换）。
  - 任一步失败都删掉目标，返回 `CopyError`（变体 `Io` / `LengthMismatch` / `Durability`，`ReadbackMismatch` 留给 US4）。
  - 不删源；删除 `move_file`。
  - `CopyOptions` 先只有 `strict_verify: bool`，US4 再接上。
- [x] T006 [US1] 在 `crates/lanrurugi-core/src/io.rs` 的 tests 里写单测：
  - 读到一半返回 0（用一个会被截短的源文件：复制开始后由另一个任务 `set_len` 截短），断言 `LengthMismatch`、目标已删、源仍在。
  - 目标目录不可写，断言 `Io`、源仍在。
  - 正常复制，断言 `CopyOutcome.blake3` 等于 `blake3::hash(内容)`。
  - 现有两个测试改成新签名。
- [x] T007 [US1] 在 `crates/lanrurugi-api/src/download_manager/mod.rs` 里：
  - 删掉 `rename_or_copy`。
  - 新增 `pub(crate) async fn place_into_library(from, to, expected_blake3: Option<&str>, opts) -> Result<PlaceOutcome, PlaceError>`。同文件系统走 `durable_rename`，返回 `moved: true`；`rename` 返回 EXDEV 或其他「跨设备」错误时走 `copy_file`，返回 `moved: false` 和 `CopyOutcome`。不删源。
  - `copy_file` 包装改成新签名。
  - `PlaceError` 映射到 T002 的 QueueError 变体。
- [x] T008 [US1] 重写 `crates/lanrurugi-api/src/download_manager/ingest.rs` 的 `catalogue_staged_file`：
  - staging → `.ingest-*` 统一调用 `place_into_library`；`keep_bytes_on_failure` 时强制复制，不改名。
  - 跨文件系统时，源一直保留到「记录写好、`dest` 已落盘」之后才删，失败分支直接删 library 副本、保留源。同文件系统改名时仍用 `put_bytes_back` 搬回。
  - `.ingest-*` → `dest` 的两处裸 `tokio::fs::rename`（修复「有记录无文件」分支和主分支）改成 `rename` 后 `sync_dir(dest.parent())`。`.ingest-*` 已在 T007 里持久化，所以这里不重复 `sync_file`。
  - 函数开头注释写清新的顺序：复制 → 校验 → 改名进桶 → sync_dir → 写记录 → 删源。
- [x] T009 [US1] `crates/lanrurugi-api/src/download_manager/ingest.rs` 的 `put_bytes_back`：跨文件系统回退分支改用 `lanrurugi_core::io::copy_file`，带长度核对与落盘；失败时保留 `file`，不删，并 warn。
- [x] T010 [US1] `crates/lanrurugi-api/src/download_manager/stream.rs:706` 的 `.part` → `download-*.ext` 改名：改为 `durable_rename`（两者都在 `staging_dir`，同文件系统）。
- [x] T011 [US1] `crates/lanrurugi-scanner/src/relocate.rs`：本地 `move_file`（:396）跨文件系统时改为 `lanrurugi_core::io::copy_file`，成功后先 `sync_dir` 目标目录再删源；同文件系统用 `durable_rename`。`:393` 的 `copy_file` 跟着改签名。
- [x] T012 [US1] 在 `crates/lanrurugi-api/src/download_manager/ingest.rs` 的 tests 里写测试：
  - 跨文件系统的情况用 `place_into_library` 的复制分支模拟（直接调复制分支的内部函数）。
  - 复制失败（目标目录只读）后：`catalogue_staged_file` 返回错误、源文件仍在原处、archive_dir 里没有 `.ingest-*` 与 bucket 文件、无记录。
  - 删源前中断（SC-001 情况 3）：手工放一个与源内容相同、但没有记录的 bucket 文件，模拟上次在写记录前中断；再对源入库，成功且只有一条记录。
  - 现有测试全部通过。
- [ ] T013 [US1] 跑 `mise run test-crate -- lanrurugi-core lanrurugi-scanner lanrurugi-api`，用 `setsid nohup` 脱离当前进程树（CLAUDE.md）。

**Checkpoint**: US1 完成后，复制与改名路径不再丢源。

## Phase 4: User Story 2 — 中断后续传不会拼出坏文件（P1）

**Goal**: 续传只从已经落盘确认的位置继续（G4）。

**Independent Test**: quickstart C。用测试服务器在中途断开后把 `.part` 尾部改成零，续传结果与原资源逐字节一致；没有 `confirmed_len` 的旧 meta 从头下载。

- [x] T014 [US2] `crates/lanrurugi-api/src/download_manager/stream.rs`：
  - `ResumeMetadata` 加 `#[serde(default)] confirmed_len: Option<u64>`。
  - 新增 `write_meta(paths, &meta)`：写 `<meta>.tmp` → `sync_all` → `rename` 覆盖，与 `lanrurugi-storage/src/sidecar.rs` 同法。
  - 首次写 meta（:526 附近）改用它。
- [x] T015 [US2] `stream.rs` 写循环，只在可续传调用（`resume_paths.is_some()`）时做：
  - 每写满 64 MiB 或距上次确认满 10 秒（先到者），`file.sync_data()`，再用 `write_meta` 写入 `confirmed_len = downloaded`。
  - 常量 `CONFIRM_EVERY_BYTES` / `CONFIRM_EVERY` 放模块顶部并写一句理由（FR-008）。
  - `sync_data` 失败视为写入失败（`DownloadError::Io`）。
- [x] T016 [US2] `stream.rs` 的 `inspect_resume_state` 与续传请求：
  - `existing_len` 改为取 `meta.confirmed_len`，没有就返回 `Fresh` 并删掉旧 `.part` 与 meta（FR-007 的「无确认点从头下载」）。
  - 发请求前，`part` 的实际长度若大于 `confirmed_len`，先用 `set_len(confirmed_len)` 截断；若小于（文件被外部截短），按 `Fresh` 处理。
  - `Range` 从 `confirmed_len` 开始；`If-Range` 逻辑不变，服务端判定内容变了时照旧整体丢弃。
- [x] T017 [US2] 在 `stream.rs` 的 tests 里写测试，复用现有本地测试服务器（`spawn_test_server`）：
  - 预置 `.part`（前 N 字节正确 + 尾部 M 字节零）和 `confirmed_len = N` 的 meta，续传后文件与原资源逐字节相等，且服务端收到 `Range: bytes=N-`。
  - meta 没有 `confirmed_len` 时服务端收到不带 `Range` 的请求。
  - 现有续传测试（`a_cancelled_download_resumes_from_where_it_left_off_on_retry` 等）保持通过：取消时也要先确认一次，否则续传会从 0 开始。所以取消、网络错误、超时这三个保留 `.part` 的分支，在返回前对已写部分做一次 `sync_data` 并写 `confirmed_len`。
- [ ] T018 [US2] 跑 `mise run test-crate -- lanrurugi-api`（detached）。

## Phase 5: User Story 3 — 每个新档案都有可核对的全文件指纹（P2）

**Goal**: 第一次经手字节时就算出 BLAKE3，入库时比对，写进档案记录（G5）。

**Independent Test**: quickstart D。下载后记录里的 `blake3` 与 `b3sum` 一致；暂存文件改一个字节后，跨文件系统入库被拒（`StagedContentCorrupted`），源保留。

- [x] T019 [US3] `crates/lanrurugi-core/src/entities.rs` 的 `Archive` 加 `#[serde(default)] pub blake3: Option<String>`，并修好所有 `Archive { .. }` 字面量（`cargo check` 会逐个指出）。`crates/lanrurugi-storage/src/repository.rs` 的处理：
  - `save`：仿照 `thumbhash`，`Some` 时才 `HSET`，`None` 时不动（不清除）。
  - 读取：`fields.remove("blake3")`。
  - 新增 `set_blake3(id, &str)`，只写这一个字段（补算任务用，避免整条回写）。
- [x] T020 [P] [US3] `crates/lanrurugi-storage/src/download_queue.rs` 的 `StagedDownload` 加 `#[serde(default, skip_serializing_if = "Option::is_none")] blake3: Option<String>`。`crates/lanrurugi-api/src/download_queue.rs` 的 `note_ingest_failure` 加一个 `blake3: Option<&str>` 参数并写入；`reingest` 把它带回 `DownloadedFile`。
- [x] T021 [US3] `crates/lanrurugi-api/src/download_manager/stream.rs`：
  - `DownloadedFile` 加 `blake3: Option<String>`。
  - 写循环每块 `hasher.update`。
  - 续传时，在发请求前把 `[0, confirmed_len)` 读一遍重建 hasher（research R3）。
  - 完成后 `blake3 = Some(hex)`。
  - 修好所有 `DownloadedFile { .. }` 构造点：`upload.rs`、`bundle.rs`、`download_queue.rs`、测试。
- [x] T022 [P] [US3] `crates/lanrurugi-api/src/download_manager/bundle.rs`：zip 输出包一层 `HashingWriter`（`Write` 包装，每次 `write` 都 `update`），放在 `BufWriter` 外层，计算的就是落盘字节；`DownloadedFile.blake3` 取其结果。
- [x] T023 [P] [US3] `crates/lanrurugi-api/src/upload.rs`：
  - `stream_field_to_file` 在 SHA-1 旁边同时算 BLAKE3。
  - 分片上传完成时的 `sha1_of_file` 改为一次读同时算 SHA-1 与 BLAKE3。
  - `StagedUpload` 带上 blake3，传进 `DownloadedFile`。
- [x] T024 [US3] `crates/lanrurugi-scanner/src/pipeline.rs`：
  - `IngestOptions` 加 `blake3: Option<&'a str>`。
  - `catalogue_new_archive`（以及 Rekeyed / Overwrite 这些新建或替换记录的分支）在保存前定好 `archive.blake3`：调用方给了就用；没给就在已有的阻塞上下文里读一遍文件算（监视器发现、分卷拆分产物）。读失败时入库返回错误，不保存没有指纹的新记录。
  - `IngestOptions::named` 等构造器默认 `None`；更新所有 `IngestOptions { .. }` 字面量。
- [x] T025 [US3] `crates/lanrurugi-api/src/download_manager/ingest.rs`：
  - `catalogue_staged_file` 加参数 `expected_blake3: Option<&str>`。
  - 走复制分支时，`CopyOutcome.blake3` 与基准不一致就删 `.ingest-*`，返回 `IngestDownloadError::StagedCorrupted`（映射到 `QueueError::StagedContentCorrupted`），不删源。没有基准时，用复制算出的值作为指纹。
  - 走同文件系统改名时，直接沿用基准（FR-013）。
  - 最终把指纹传给 `IngestOptions.blake3`。
  - 调用方改动：`ingest_staged_download`（`DownloadedFile.blake3`）、`resolve_overwrite` / `resolve_rename` 传 `None`；`adopt_pending_conflict` 传 `None`，复制时得到指纹。
- [x] T026 [US3] `crates/lanrurugi-api/src/plugins.rs` 的两处 `note_ingest_failure` 传 `DownloadedFile.blake3`。同时处理 `StagedCorrupted` 失败：只对下载来源生效，不调用 `note_ingest_failure`（暂存文件已坏，不保留），并删除暂存文件，让队列行的重试走重新下载（R7）。`crates/lanrurugi-api/src/download_queue.rs` 的 `reingest` 遇到同一错误时，也删暂存文件、清空 `staged`、错误写 `StagedContentCorrupted`。「需要重新下载」不是新的队列状态，而是 `Error` 状态 + 这个错误 + 没有 `staged`，现有的重试按钮就是重新下载。
- [x] T027 [US3] `crates/lanrurugi-api/src/share_net/import.rs:1083`：传入 `Some(hex(&root))` 作为基准（bao 根哈希就是文件的 BLAKE3，`hash_file` 已在 FR-022 校验过一次）。
- [x] T028 [US3] `GET /api/archives/{id}/metadata` 输出可选字段 `blake3`（`crates/lanrurugi-api/src/archives.rs` 的序列化处），并在 `docs/api/sidecar/` 补字段说明。跑 `mise run docs-api -- --check`。
- [x] T029 [US3] 备份：`crates/lanrurugi-backup/src/build.rs` 与 `restore.rs` 若逐字段搬档案记录，加上 `blake3`；若整体序列化 `Archive`，则确认 `serde(default)` 让旧备份能读入。在对应的测试里断言 round-trip。
- [x] T030 [US3] 测试：
  - `ingest.rs` tests：给一个错误的基准走复制分支，得到 `StagedCorrupted`、源仍在、无记录；给正确的基准，记录 `blake3` 等于 `blake3::hash(内容)`。
  - `stream.rs` tests：正常下载与续传下载的 `DownloadedFile.blake3` 等于整个资源的哈希。
  - `bundle.rs` tests：输出文件的哈希等于 `DownloadedFile.blake3`。
  - `pipeline.rs` tests：不给基准时记录也有正确的 `blake3`。
- [ ] T031 [US3] 跑 `mise run test-crate -- lanrurugi-core lanrurugi-storage lanrurugi-scanner lanrurugi-api lanrurugi-backup`（detached）。

## Phase 6: User Story 4 — 可选的严格校验（P3）

**Goal**: 打开后，复制时由独立任务绕过缓存读回刚写下的块并比对；默认关闭（FR-014–016）。

**Independent Test**: quickstart E。默认无回读；打开后在 ext4/xfs 上回读确实发生；替身让回读不一致时入库失败、源保留；ZFS 上记录跳过原因。

- [x] T032 [US4] `crates/lanrurugi-core/src/io.rs`：`copy_file` 在 `strict_verify` 时：
  - 每块写完后，把 `(offset, len, blake3(chunk))` 送进容量为 3 的 `tokio::sync::mpsc`（FR-015）。
  - 校验任务在 `spawn_blocking` 里用独立句柄按偏移读回比对：Linux/FreeBSD 用 `O_DIRECT`，读缓冲按 4 KiB 对齐；macOS 用 `F_NOCACHE`。
  - 写完全部块后先 `sync_file`，再等校验任务结束。
  - 有不一致时返回 `ReadbackMismatch { offset, len }`，目标删除。
  - 读回函数抽成 trait，测试用替身注入。
- [x] T033 [US4] `crates/lanrurugi-core/src/io.rs`：新增 `can_bypass_cache(dir) -> Result<(), &'static str>`。识别 ZFS：Linux 上看 `statfs` 的 `f_type == 0x2fc12fc1`，FreeBSD 和 macOS 上看 `statfs.f_fstypename == "zfs"`，命中则返回「ZFS 无法绕过 ARC」；其他文件系统照常尝试；`O_DIRECT` 打开失败（EINVAL）也返回原因。不能绕过时只 `tracing::info!` 一次原因并跳过回读（FR-016）。原因字符串放进 `CopyOutcome.readback_skipped: Option<&'static str>`。
- [x] T034 [US4] 设置项 `ingest_strict_verify`：
  - `crates/lanrurugi-api/src/settings.rs` 的 `BOOL_FIELDS`，默认 `false`。
  - `place_into_library` 读取它并传入 `CopyOptions`。
  - 前端设置页「下载」区加开关和说明：本地 ZFS 上无效，HDD 上会变慢，适合 NFS/SMB 书库。文件：`apps/frontend/src/pages/Settings/` 下下载设置所在的文件，以及 `apps/frontend/src/api/types.ts` 的 `Settings` 类型，4 个 locale。
- [x] T035 [US4] 测试（`io.rs`）：
  - 关闭时不回读，用替身计数。
  - 打开时，替身对第 2 块返回不同数据，得到 `ReadbackMismatch`，目标删除、源在。
  - 打开时，在 tmpfs/ext4 的 `std::env::temp_dir()` 上真实回读通过；若该环境 `O_DIRECT` 不可用，断言 `readback_skipped` 有原因。
- [ ] T036 [US4] 跑 `mise run test-crate -- lanrurugi-core lanrurugi-api`（detached），前端跑 `tsc` 与 `eslint`。

## Phase 7: User Story 5 — 指纹维护与存量补算（P3）

**Goal**: 记录建立或替换时指纹始终是新的（T024 已覆盖）；旧档案可手动补算（FR-017/018）。

**Independent Test**: quickstart F。补算任务在任务控制台可见、可取消，完成后不再有无指纹的档案。

- [x] T037 [US5] 新建 `crates/lanrurugi-api/src/integrity_backfill.rs`：
  - `POST /api/database/blake3_backfill` 启动一个 `lanrurugi_core::jobs` 任务。
  - 遍历没有 `blake3` 的档案，按批交给受限 rayon 池（复用 `recommend_precompute::precompute_worker_budget`），并按 `LoadThrottle` 退避。
  - 每个档案读一遍算 BLAKE3，再 `set_blake3`。
  - 进度写入任务，检查取消。
  - 文件缺失或读失败的档案计数后跳过。
  - 路由注册与 `route_policy.csv`：`token_guest` / `guest_visitor` deny。
  - 记一条活动日志（参照现有 database 维护操作的 action type）。
- [x] T038 [US5] 前端：设置页维护区加「补算档案指纹」按钮（参照现有「重建索引」类按钮），启动后提示去任务控制台；4 个 locale。
- [x] T039 [US5] relocate 保持指纹：确认 `crates/lanrurugi-scanner/src/relocate.rs` 更新记录的方式（读出再保存，或只改 `file`）不会丢掉 `blake3`；在其测试里加断言。
- [ ] T040 [US5] 测试：补算任务在测试 Redis 上，对两个无指纹档案补齐，且值正确；取消后不再继续。跑 `mise run test-crate -- lanrurugi-api lanrurugi-scanner`（detached）。

## Phase 8: Polish & Cross-Cutting

- [ ] T041 `mise run fmt`、`mise run clippy`、前端 `tsc --noEmit` 与 `eslint`、`mise run docs-api -- --check`。
- [x] T042 [P] `README.md` 的「能做什么」与 `docs/comparison-with-lanraragi.md`：补一条「入库完整性：复制校验、落盘顺序、续传确认点、全文件 BLAKE3」。
- [ ] T043 在 dev 容器（`mise run dev-rebuild && dev-down && dev-up`，detached）上做接口级验收：
  - 正常下载或上传一个档案，`metadata.blake3` 与 `b3sum` 一致（D1）。
  - 补算任务跑通（F2）。
  - 默认设置下没有回读日志（E1）。
- [ ] T044 在 tasks.md 末尾写 Implementation Record：
  - 哪些只在单测里验证。
  - 哪些在 dev 容器里验证。
  - 需要真机做的（NAS 上的 SC-003 / SC-006 实测、跨文件系统断电），留给用户。
  - 与 plan.md 的偏差。
  - 全量 `mise run test` 由 CI 跑。

## Dependencies & Execution Order

- Phase 2（T002–T004）阻塞所有故事。
- US1（T005–T013）是 US3 和 US4 的前提：`copy_file` 的新签名和 `place_into_library` 都在 US1 里落地。
- US2（T014–T018）只依赖 Phase 2，可以和 US1 并行。T021 的续传重建 hasher 依赖 US2 的 `confirmed_len`。
- US3：T019 → T024 → T025 → T026/T027。T020/T022/T023 改的是不同文件，可以并行。
- US4 依赖 US1 的 `copy_file`。US5 依赖 T019 的 `set_blake3`。

## Parallel Example

```text
US3: T020 (download_queue.rs) ‖ T022 (bundle.rs) ‖ T023 (upload.rs)，然后 T024 → T025
```

## Implementation Strategy

1. **MVP**：Phase 2 + US1 + US2，即 plan 的 M1–M3，P1 防丢数据，可以单独进 beta6。
2. 接着做 US3（M4），之后的巡检和共享都依赖它。
3. US4、US5 默认关闭或手动触发，最后做。
