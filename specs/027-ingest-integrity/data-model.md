# Data Model: 入库完整性

所有新增字段都是可选的；旧数据缺字段时按「没有指纹 / 没有确认点」处理（宪法原则 I：不做破坏性迁移，档案 id 不变）。

## Archive（`crates/lanrurugi-core/src/entities.rs:21`）

| 字段 | 类型 | 说明 |
|---|---|---|
| `blake3` | `Option<String>`（64 位小写十六进制） | 全文件 BLAKE3。Redis 档案 hash（DB 0）新增同名字段；旧记录没有该字段。更新时只 `HSET` 这一个字段，不整条回写 |

- 只增不改：`id` 仍由 `size_aware_id` / `legacy_id` 计算。
- 改写档案字节的操作（分卷拆分、补页、覆盖式重复处理）完成后必须同步更新（FR-017）。
- legacy LANraragi 读同一个 Redis 时会忽略未知字段。

## StagedDownload（`crates/lanrurugi-storage/src/download_queue.rs:135`，队列行的 `staged` 字段）

| 字段 | 类型 | 说明 |
|---|---|---|
| `blake3` | `Option<String>` | 入库基准指纹。入库失败、字节被保留时，由 `note_ingest_failure` 写入；`reingest` 以它为比对基准。`#[serde(default)]` |

不在 `DownloadQueueItem` 上另加 `staged_blake3`：下载完成后在同一进程里立即入库，能跨重启保留下来的「已下载未入库」字节只有 `staged` 这一处。`PendingFilenameConflict` 是 020 之前的遗留数据，不再新建，接管时由复制现算指纹，不加字段。

## ResumeMetadata（`crates/lanrurugi-api/src/download_manager/stream.rs:54`，`.meta.json`）

| 字段 | 类型 | 说明 |
|---|---|---|
| `confirmed_len` | `Option<u64>` | 已 `sync_data` 的字节数。缺失 → 部分文件不可信，从头下载 |

写入方式：写 `<meta>.tmp` → `sync_all` → 改名覆盖（与 `lanrurugi-storage/src/sidecar.rs:109-111` 一致）。

## QueueError（`lanrurugi_core::queue_error::QueueError`）新增变体

| 变体 | 字段 | 触发 |
|---|---|---|
| `CopyLengthMismatch` | `path`, `expected`, `copied`, `dest_len` | FR-001 |
| `DurabilityFailed` | `path`, `os_error` | FR-002/FR-003 中任一 fsync 失败 |
| `StagedContentCorrupted` | `path`, `expected_blake3`, `actual_blake3` | FR-012；下载来源的行据此提示「需要重新下载」 |
| `VerifyReadbackMismatch` | `path`, `offset`, `len` | FR-014 严格校验 |

每个变体在 en / zh / zh_Hant / ja 四个主维护 locale 有对应文案，其余 locale 回退 en。

## 设置

| 键 | 存放 | 默认 | 说明 |
|---|---|---|---|
| `ingest_strict_verify` | Redis `LRR_CONFIG`（布尔字段，进 `settings.rs` 的 `BOOL_FIELDS`） | `false` | FR-014 |

## 状态与顺序（复制路径）

```
copy (hash every chunk) ─► len check ─► hash == staged.blake3? ─► sync_all(dest)
   │ fail: remove .ingest-*, keep source         │ no: StagedContentCorrupted
   ▼                                              ▼
rename .ingest-* → bucket ─► fsync(bucket dir) ─► remove source ─► save record (blake3)
```
