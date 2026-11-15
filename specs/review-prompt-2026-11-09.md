# 给 Claude Code 的 review 提示（PR #258，2026-11-09）

把下面「提示正文」整段贴给 Claude Code（在 `/home/jade/LANrurugi` 工作区里，分支
`feat/share-bind-interface`）。它的目标不是复述我做了什么，而是**挑错**。

---

## 提示正文

你在 review 一个尚未合并的 PR：`/home/jade/LANrurugi` 的 `feat/share-bind-interface`（PR #258），
相对 `origin/dev` 有 49 个 commit、约 290 个文件。它包含四份 spec 的实现，
`specs/020-hash-named-library`、`specs/021-direct-download-domain`、
`specs/022-share-network-hardening`、`specs/023-share-crate-extraction`。

请按下面的顺序做，最后给出一份按严重程度排序的问题清单（每条都带文件:行号与复现/推理依据）。

1. **先读四份 spec 的 `spec.md` 与 `tasks.md` 的 Implementation Record。** 那里写了实现者自己
   声明的偏离（例如 020 没有给 `Archive` 加 `original_filename` 字段、`.classic` 改名成 `_byname`、
   022 的 T038 故意保留 `opt-level = 2`、023 明确是部分实现）。偏离本身不算问题，
   **偏离了但没写、或写了但代码没做到**，才算。

2. **重点看 020 的数据一致性**。这是最容易悄悄丢数据的一块：
   - `crates/lanrurugi-scanner/src/relocate.rs`：把非哈希布局的档案搬进 `_byname/`，包括 60 秒静默窗口、
     `EXDEV` 回退（copy → fsync → rename → unlink）、目标被占时跳过、`.patch.zip` 随行。
     请检查：搬家中途进程被杀会留下什么状态？`LRR_FILEMAP` 的旧键是否可能残留导致后续扫描认错文件？
     补丁搬失败但档案搬成功时，库里是否出现「档案在 `_byname/`、补丁在原地」的不可发现状态？
   - `crates/lanrurugi-scanner/src/pipeline.rs` 的 `catalogue_new_archive`：读旁车恢复标题、写旁车。
     哈希布局与 `_byname` 两种路径是否都正确？旁车写失败只 `warn!` 时，记录与旁车是否会永久分叉？
   - `crates/lanrurugi-api/src/download_manager/ingest.rs` 的 `catalogue_staged_file`：
     先复制到 `.ingest-<uuid>` 再算 id 再 rename。请检查三条分支（新档案 / 记录在但文件丢 / 记录与文件都在）
     是否都在失败时留下可恢复状态，尤其是 rename 跨文件系统失败、以及 `DuplicatePolicy::Overwrite` 时。
   - `crates/lanrurugi-api/src/download_manager/ingest.rs::adopt_pending_conflict` 与
     `crates/lanrurugi-server/src/main.rs` 的启动修复块：旧的「等待改名」队列项会被按内容收编。
     如果暂存文件其实是半截下载，会发生什么？

3. **看 021 的信任边界**。`crates/lanrurugi-api/src/download_url.rs` 与
   `crates/lanrurugi-api/src/download_host.rs`、`crates/lanrurugi-api/src/direct_download.rs`：
   - 签名链接的 TTL、HMAC 覆盖的字段、以及「下载域只放行 `/api/dl/` 与 `/api/share/r/`」是否
     真的能被绕过（例如用 `..`、大小写、百分号编码、双斜杠）。
   - `settings.rs` 的 scheme 校验以 `share_public_url` 为页面 origin；如果用户从没设置过公开地址，
     这个校验是否形同虚设？那是不是应该拒绝而不是放过？

4. **看 022 的 fork 是否只改了一件事**。`vendor/netwatch`、`vendor/iroh-relay` 各有一个 `FORK.md`，
   声称只加了一个 `SO_BINDTODEVICE` 选项。请用 `git diff --no-index` 对照上游
   （`~/.cargo/registry/src/*/netwatch-0.19.3`、`iroh-relay-1.4.1`）确认没有别的改动，
   并检查 `set_bind_device` 的进程级静态变量在「同时有两个端点/两次 bind 交替」时是否会串味。
   另外看 `crates/lanrurugi-api/src/share_net/mod.rs` 的 `start()`：中继复用（`kept_relay`）在
   「端点身份变了但中继设置没变」时把旧中继交给新端点，这是否会让旧 endpoint id 仍被中继接受？

5. **看 022 的重试语义**。`crates/lanrurugi-api/src/share_net/import.rs` 现在把网络错误当作
   「无限等待」，只有分享方的明确拒绝才是终态。请检查：
   - `verified: Result<(), (bool, String)>` 里那个 `bool` 的用法是否每一处都对（`true` = 终态）；
   - 无限重试 + 60 秒上限的退避，是否可能让一个**已被删除**的分享永远占着导入任务；
   - `crates/lanrurugi-api/src/plugins.rs` 的下载重试（4 次、2/4/8/16 秒）与它判定的错误类型是否一致；
   - `stream.rs` 的 180 秒空闲超时在「服务器正常但很慢」时会不会误杀。

6. **看 023 是否只是搬家**。`crates/lanrurugi-share` 目前是部分实现，`ShareHost` 在
   `crates/lanrurugi-share/src/host.rs`，`AppState` 的实现点在
   `crates/lanrurugi-api/src/share_host.rs`。请检查搬走的模块（`interfaces`/`link`/`status`/`proxy`/
   `settings`/`limits`/`blobs`）在行为上是否与搬之前完全一致（尤其 `limits` 从异步变同步、
   `blobs` 从 `&AppState` 变 `&dyn ShareHost` 之后，有没有把错误吞掉或改变了并发语义）。

7. **跑一遍门禁，并相信你自己看到的结果**：
   ```sh
   mise run fmt-check && mise run clippy
   mise run check-crate -- lanrurugi-api lanrurugi-scanner lanrurugi-share
   podman run -d --rm --name lrr-test-redis -p 16380:6379 docker.io/library/redis:7-alpine --databases 64
   LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16380 mise run test-crate -- lanrurugi-api lanrurugi-scanner lanrurugi-storage
   cd apps/frontend && npx --no-install tsc --noEmit && npx --no-install eslint src
   ```
   注意：本机 `mise run test`（全量）在编译 `lanrurugi-book-worker` 时会被 OOM 杀掉，这不是测试失败；
   全量请交给 CI。

8. **不要做的事**：不要合并 PR、不要改代码后直接推、不要开 issue。如果发现问题，只报告，
   带 `文件:行号` 和最小复现思路；改动由人来决定。

输出格式：

- **阻断级**（会丢数据/泄漏/崩溃）
- **应修**（行为与 spec 不符、边界未处理）
- **可延后**（风格、命名、注释）
- 每条：`文件:行号` → 现象 → 为什么是问题 → 建议的最小验证方式。
