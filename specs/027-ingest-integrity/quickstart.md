# Quickstart: 入库完整性验收

前提：dev 容器（`mise run dev-rebuild && dev-down && dev-up`），书库挂载点与暂存区不在同一文件系统。先用 `stat -f` 分别看两处的文件系统确认；如果相同，把暂存目录指到一个 tmpfs 来制造跨文件系统。

## A. 复制少了不删源（US1，SC-001）

1. 单测：`lanrurugi-core` 中用一个读到一半返回 0 的替身 reader 调 `copy_file`，断言 `LengthMismatch`、目标被删除、源仍在。
2. 单测：替身 writer 在 `sync_all` 时返回 `ENOSPC`，断言 `Durability` 错误、源仍在。

## B. 删源前中断（US1）

1. 在 `rename_or_copy` 的「改名后、删源前」加测试钩子令任务 panic。
2. 重启后对同一文件再次入库：成功、无重复档案、`.ingest-*` 被 `sweep_stale_ingest_copies` 清掉。

## C. 崩溃后续传（US2，SC-002）

1. 用本地 HTTP 测试服务器提供一个 200 MB 文件（支持 Range 与强 ETag）。
2. 下载到约 120 MB 时 kill 进程；把 `.part` 尾部 8 MB 覆写为零。
3. 重启并重新开始：日志显示从 `confirmed_len` 续传；最终文件 BLAKE3 与源文件一致。
4. 删除 `.meta.json` 里的 `confirmed_len` 再做一遍：从头下载。

## D. 指纹（US3，SC-004/005）

1. 正常下载一个档案：`GET /api/archives/{id}/metadata` 的 `blake3` 与 `b3sum <书库文件>` 一致。
2. 让下载完成后暂停入库（测试钩子），把暂存文件中间改一个字节，再放行：队列行 `StagedContentCorrupted`，书库无新档案。
3. 下载完成后重启服务再入库：比对基准来自 `staged_blake3`（日志可见），而非重启后现算。
4. 多资源打包（pixiv 测试插件）、网页上传、共享导入各入库一次：记录都有 `blake3`。

## E. 严格校验（US4，SC-006）

1. 默认关闭：入库时无回读日志。
2. 开启，在 ext4/xfs 临时目录上入库：日志显示回读块数；用替身令某块回读不同：`VerifyReadbackMismatch`，源保留。
3. 开启，在 ZFS 上入库：日志写明「无法绕过 ARC，跳过回读」。
4. 维护者 NAS 上实测 1.4 GB 档案开关前后耗时，记录到 tasks.md 的 Implementation Record。

## F. 指纹维护（US5）

1. 对有指纹的档案执行补页：指纹改变且与新文件一致。
2. 运行「补算档案指纹」任务：任务控制台可见进度、可取消；完成后无指纹档案数为 0。

## G. 性能（SC-003）

在 NAS 上对同一 1.4 GB 档案比较本功能前后（严格校验关闭）的入库耗时，增幅 ≤ 5%。
