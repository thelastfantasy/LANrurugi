# Contract: `lanrurugi_core::io` 与对外可见的变化

## 内部接口（`crates/lanrurugi-core/src/io.rs`）

```rust
/// What a finished copy proved about itself.
pub struct CopyOutcome {
    pub bytes: u64,
    pub blake3: blake3::Hash,       // of the bytes actually read from `from`
}

pub struct CopyOptions {
    pub strict_verify: bool,         // FR-014; default false
}

/// Copies, checks the length (FR-001), syncs the destination (FR-002). Does not remove `from`.
pub async fn copy_file(from: &Path, to: &Path, opts: &CopyOptions) -> Result<CopyOutcome, CopyError>;

/// Durable same-filesystem move: sync `from`, rename, sync the target directory (FR-003).
pub async fn durable_rename(from: &Path, to: &Path) -> std::io::Result<()>;

/// `fsync` on a directory (F_FULLFSYNC on macOS where available).
pub async fn sync_dir(dir: &Path) -> std::io::Result<()>;

pub enum CopyError {
    Io(std::io::Error),
    LengthMismatch { expected: u64, copied: u64, dest_len: u64 },
    Durability(std::io::Error),
    ReadbackMismatch { offset: u64, len: u64 },
}
```

- `move_file` 删除：唯一调用方 `download_manager::rename_or_copy` 改为显式「复制 → 比对指纹 → 改名 → sync_dir → 删源」，因为指纹比对必须发生在删源之前，而比对基准只有调用方知道。
- `relocate.rs:393` 改用新 `copy_file`（拿到长度与持久化保证；指纹可写回记录，替代补算）。

## 对外可见

- `GET /api/archives/{id}/metadata`：新增可选字段 `blake3`（字符串或缺省）。新增字段对 legacy 客户端无影响（宪法原则 II：只增不改）。docgen 的 sidecar 需补这一字段说明，`mise run docs-api -- --check` 必须通过。
- 下载队列行（`GET /api/download_queue`）的 `error` 可能出现 data-model.md 列出的四个新错误类型。
- 设置页「下载」区新增「严格校验」开关及说明（含「本地 ZFS 上无效」「HDD 上会变慢」）。
- 任务控制台新增任务类型「补算档案指纹」，入口在设置页维护区。
