# Research: 入库完整性

## R1. 是否引入 `blit-core`

- **Decision**: 不引入。
- **Rationale**: crates.io 上只有 0.1.2 一个版本（2026-08-18），总下载 82 次，单人维护；约 5.5 万行、46 处 `unsafe`；依赖 `tonic`/`prost`/`mdns-sd`/`sysinfo`/`tar`/`md5`/`xxhash` 等与本需求无关的组件。它的 `copy::file_copy::copy_file` 不做复制后校验，`checksum.rs` 是 rsync 风格的滚动校验（服务于增量同步）。同步 `std::fs` + `eyre`，与本项目的 tokio + `thiserror` 不一致；015 要求的 FreeBSD/macOS 构建和 `cargo deny` 许可证门禁都会增加负担。
- **Alternatives**: 直接用工作区已有的 `blake3 = "1.8.7"`（共享导入已在用，`share_net/import.rs:678`）。

## R2. 指纹算法

- **Decision**: BLAKE3，十六进制 64 字符存档案记录。
- **Rationale**: 宪法原则 III 要求新哈希工作优先 BLAKE3；已是依赖；单核吞吐远高于 NAS 磁盘与网络，可以在复制循环里内联计算而不成为瓶颈；与共享导入的指纹是同一种，可直接复用。
- **Alternatives**: SHA-256（慢数倍，无额外收益）；xxh3（非密码学，无法作为跨实例共享的内容凭证）。

## R3. 指纹在何时计算（不额外读盘）

- **Decision**: 在字节第一次被 Rust 经手时增量计算：托管下载在 `stream.rs` 写 `.part` 的循环里；打包在 `bundle.rs` 的 zip 输出流上（包一层 hashing writer）；上传在接收请求体时；共享导入沿用 iroh 已校验的根哈希对应的文件哈希。插件自行下载（`file_path`）、启动接管、监视器发现的文件只能在接手时读一遍。
- **Rationale**: 复制循环本来就要读一遍源；下载循环本来就要写一遍；只有「Rust 从未经手」的路径才需要额外读。
- **续传时的增量状态**: `blake3::Hasher` 不可序列化。恢复时对确认点之前的前缀重读一次重建 hasher——只在续传时发生，且只读已确认部分。替代方案「持久化 Hasher 内部状态」需要依赖库私有布局，不采用。

## R4. 持久化（fsync）与顺序

- **Decision**:
  - 复制：写完 → `sync_all()` 目标文件 → 改名进桶 → `fsync` 目标目录 → 删源。
  - 同文件系统改名：`sync_all()` 源文件 → `rename` → `fsync` 目标目录。
  - macOS 用 `fcntl(F_FULLFSYNC)`（`libc` 已是工作区依赖），失败时退回 `fsync`；Linux/FreeBSD 用 `fsync`。目录 fsync 用 `File::open(dir)?.sync_all()`（Linux/FreeBSD/macOS 均可）。
  - 代码落点：`lanrurugi_core::io` 新增 `durable_rename`、`sync_dir`，`copy_file` 内部在返回前 `sync_all`。
- **Rationale**: G2/G3。NFS/SMB 的延迟写错误只有 fsync 或 close 时才可靠报告；tokio 的 `File` drop 时不报告 close 错误，所以必须显式 `sync_all` 并检查结果。
- **成本**: 每个档案一次文件 fsync + 一到两次目录 fsync，相对复制 GB 级文件可忽略（SC-003 实测确认）。

## R5. 续传确认点

- **Decision**: 下载循环每写满 64 MiB 或每 10 秒（先到者）`sync_data()` 一次 `.part`，随后把 `{confirmed_len}` 写回 `.meta.json`（先写临时文件再改名，与 `sidecar.rs:109-111` 同法）。续传时：`set_len(confirmed_len)` 截断 → 以 `confirmed_len` 发 `Range`。无 `confirmed_len` 字段（升级前的 meta）→ 删除部分文件从头下载。
- **Rationale**: G4。确认点之后的数据在崩溃后不可信；之前的数据已经 `sync_data` 过。64 MiB/10 s 让 fsync 次数在 GB 级下载中是几十次量级。
- **Alternatives**: 每块 fsync（吞吐损失大）；只在完成时 fsync（不能解决崩溃中途的问题）；对 `.part` 做逐块哈希列表（能定位坏块，但续传本来就只需要一个安全前缀，复杂度不值）。

## R6. 严格校验（流水线回读）

- **Decision**: 默认关闭。开启时，复制循环每写完一块，把 `(offset, len, blake3(chunk))` 送入容量为 3 的 `tokio::sync::mpsc`；校验任务用独立文件句柄在 `spawn_blocking` 里按偏移读回该块并比对。读回必须绕过缓存：Linux/FreeBSD 用 `O_DIRECT`（对齐缓冲区与偏移，块大小已是 4 KiB 的整数倍），macOS 用 `F_NOCACHE`。
- **何时判定「无法验证磁盘」**: `statfs` 识别到 ZFS（Linux `f_type == 0x2fc12fc1`）时，除非 OpenZFS ≥ 2.3 且数据集 `direct=always/standard` 可确认，否则视为无法绕过 ARC，跳过并记录（FR-016）。打开 `O_DIRECT` 失败（EINVAL）同样跳过并记录。
- **Rationale**: 刚写入的块在页缓存/ARC 中，普通读回只是内存对内存；网络挂载（NFS）上 `O_DIRECT` 读会直接向服务器取，能验证网络写入段，这是本功能对 NFS/SMB 用户的真正价值。HDD 上真读盘会与顺序写交错寻道，所以默认关闭并要求实测（SC-006）。
- **不用 rayon**: IO 密集；CPU 部分（BLAKE3 一块 4 MiB）在 tokio blocking 线程里足够。
- **Alternatives**: 复制完整体回读（无法与复制重叠，耗时翻倍）；`posix_fadvise(DONTNEED)` 后普通读（Linux only、对脏页无效、需先 fsync 每块）。

## R7. 指纹不一致时的处理

- **Decision**: 复制端指纹 ≠ 基准 → 判定暂存文件损坏（复制读到的就是暂存文件的内容）。下载来源：删除 `.ingest-*` 副本，队列行置为「需要重新下载」并清除暂存文件；接管来源（`keep_bytes_on_failure`）：保留原文件，队列行报错，不删除、不重下载。
- **Rationale**: 从坏的暂存文件重试复制没有意义；接管路径的文件是用户唯一副本。

## R8. 存量补算

- **Decision**: 新增后台任务「补算档案指纹」，走 `lanrurugi_core::jobs`，按 CLAUDE.md 的规则限制并发（约 30% 核心）并带 `LoadThrottle` 退避，可取消；每个档案读一遍算 BLAKE3 后只 `HSET` 指纹字段（避免整条回写覆盖并发修改，参见 024 复查里 `tidy_tags` 的问题）。
