# 复查结果（2026-11-10）：对 c 轮修复（`d9ebce9` `61fd2f9` `7e3addb`）的核对

分支 `feat/share-bind-interface`，HEAD `7e3addb`。`gh run list` 显示 CI 为 success。这一轮我没有在本地重跑门禁，结论来自读 diff。只报告，没有改代码。

## c 轮各项的状态

| c 轮条目 | 结论 |
|---|---|
| 阻断 1：传输中断被判为终态 | **已修**。`read_frame`、`write_frame`、`finish`、`read_body` 和 truncated 都改成了 `Unreachable`，只有解码失败还是 `Protocol`。没有加「body 截断 → 可重试」的测试。 |
| 阻断 2：搬家回滚 | **已修**。清理空目录改到保存成功之后；`move_back` 会重建父目录、走 `move_file`，并把补丁一起搬回。没有加回滚测试。另外，进程在「move 之后、save 之前」被 kill 的窗口仍然存在，见可延后 3。 |
| 应修 1：macOS 绑接口 | **后端已修**：`BindingScope` 分得清楚，非 Linux 保存时返回 400，EINPROGRESS 已经按平台取值。前端有一处连带问题，见下面应修 1。 |
| 应修 2：并发入库 | **原问题已消除**，但引入了新问题，见应修 2。 |
| 应修 3：启动收编 | **原问题已消除**，改成复制入库，失败时原件还在，冲突信息也保留。但失败分支多了一步有害的回搬，见应修 3。 |
| 应修 4：缩略图 | **已修**，恢复为 `webp()`。 |
| 应修 5：搬家口径 | **已修**，新增 settling 和 outside 两个计数，补丁搬不动时算 `failed`。 |
| 可延后 1、2、3、5、6、7、9 | 已收。可延后 4 的注释也已更正，并在 Rejected 分支加了清理。 |

---

## 应修

### 1. `apps/frontend/src/pages/Settings/SharingSection.tsx:371`：非 Linux 上把整个「网络」面板都换成了说明文字，`share_portmap` 开关也跟着不见了

- **现象**：`NETWORK_FIELDS` 里有两项：`share_bind_interface` 和 `share_portmap`（`:145-162`）。当 `share === "none"` 时，代码替换的是整个 `ShareSettingsForm`，所以 macOS 和 FreeBSD 用户再也看不到 UPnP/NAT-PMP 端口映射的开关。
- **为什么**：portmap 和接口绑定没有关系，在所有平台上都有效。这是一次功能退化。
- **最小验证**：用 macOS 构建，或在本地把 `interface_binding.share` mock 成 `"none"`，打开设置 → 共享 → 网络，会发现找不到 portmap 开关。
- **修法供参考**：只把 `share_bind_interface` 这一项换成说明文字，`share_portmap` 照常渲染。

### 2. `crates/lanrurugi-api/src/download_manager/ingest.rs:409-419`：`InFlight` 路径会把字节遗留在 `.ingest-*`，而且 3 秒的等待窗口偏短

- **现象**：
  - 等待 30 × 100 ms 之后，代码直接执行 `return Err(IngestDownloadError::InFlight)`，没有处理 `incoming`。
  - 对普通下载（`keep_bytes_on_failure=false`），`staging` 前面已经被 `rename_or_copy` 搬进了 `incoming`。所以这份字节只剩书库根目录下的 `.ingest-xxx`，要等下次启动、并且过了 6 小时才会被 sweep 删掉。
  - 枚举注释写的是「kept in place so a retry can pick it up」，但下载重试会重新抓取，没有任何流程会去认领 `.ingest-*`。
  - 这个错误映射成了 `QueueError::Internal`（`:95`），用户看到的是一个看不出原因的失败。实际上另一方多半马上就会入库成功。
- **为什么**：先到的那方要跑完 `ingest_file_with_policy` 才写记录，中间包括读页数、PDF/EPUB 的 inspect 等步骤，大文件很容易超过 3 秒。所以「同一份字节同时进来两次」会稳定地得到一个 Internal 失败，外加一份要等几个小时才清理的副本。
- **最小验证**：同时对同一份较大的 zip 跑两个 `catalogue_staged_file`，并在第一个的 `ingest_file_with_policy` 里人为加一段 5 秒的延迟。预期第二个返回 `InFlight`，书库根目录下多出一个 `.ingest-*`，原来的 staging 已经不存在。
- **修法供参考**：返回 `InFlight` 前把 `incoming` 搬回 `staging_path`，让重试真的能用上它；或者干脆删掉 `incoming`，因为另一方持有同一份字节。等待时间可以放宽，也可以改成「等到 `dest` 消失或记录出现为止」。队列侧最好把这种情况显示为可重试，而不是 Internal。

### 3. `crates/lanrurugi-api/src/download_manager/ingest.rs:163-172`：收编失败时把 `dest` 搬回 `staging`，这一步多余，有时还有害

- **现象**：`keep_bytes_on_failure=true` 时，`staging` 是复制过去的，本身一直都在。Err 分支却在 `dest` 存在时执行 `rename(dest, staging)`。`dest` 在两种情况下并不是「这次收编留下的孤儿」：
  1. **记录已经写了，但之后的步骤失败了**：`catalogue_new_archive` 在 `archives.save` 之后，还有缩略图回写的 `save(...)?`，调用方也还有 FILEMAP 的 `?`。这时 `:453` 因为 `recorded` 为真而保留了 `dest`，紧接着又被这一步搬走。结果是记录指向一个不存在的文件，这正是之前反复出现的那种状态。下次启动时，`pending_filename_conflict` 还在，「record without bytes」分支会把它修好，所以能自愈；但在下次启动之前，这条记录一直指向空气。
  2. **`InFlight`**：`dest` 属于另一个正在进行的入库，这一步会把它抢到 `temp_path`，导致对方的记录也指向空气。
- **为什么**：这一步回搬原本是为「原件已经被搬走」的旧语义准备的。改成复制之后它已经没有必要，留着只会带来上面两种坏结果。
- **最小验证**：
  - 情况 1：在 `catalogue_new_archive` 的第二次 `save` 处注入错误（或者在缩略图生成完成后断开 Redis），然后调用 `adopt_pending_conflict`，断言 `archives.get(id).file` 指向的文件存在。现在会失败。
  - 现有测试 `a_failed_adoption_keeps_the_staged_file_where_the_row_points` 用一个目录占住 `dest`，走的是 `InFlight` 路径，而且那次 rename 本身就会失败，所以覆盖不到上面两种情况。
- **修法供参考**：删掉 `:163-172` 的回搬。真正需要清理的只有「没有记录、并且 `dest` 是本次 rename 进去的」那份孤儿，在 `catalogue_staged_file` 内部用 `!recorded` 判断后删除即可。

---

## 可延后

1. **`crates/lanrurugi-api/src/share_net/mod.rs:469`（以及 `:323`、`:348`）**：运行时不看 `share_binding_scope()`。如果非 Linux 主机上已经存了 `share_bind_interface`，比如是旧版本保存的，或者从 Linux 实例的备份恢复过来的，引擎仍会只绑那块网卡的地址，网卡消失时共享会一直停着。而 UI 已经没有入口可以清掉这个值。建议在运行时，scope 为 `None` 时把它当空值处理，同时打一条 warn。
2. **`crates/lanrurugi-core/src/http_client.rs:140`**：`interface_binding_supported()` 已经没有任何调用方，也没有 doc；`bind_outbound` 原来的 doc 在这次整理中被删了，没有补回去。建议删掉前者、补回后者。
3. **`crates/lanrurugi-scanner/src/relocate.rs:110` → `:256`**：「move 完成、save 还没完成」时进程被 kill，仍然会留下记录指向旧路径、文件已经在 `_byname/` 的状态，目前启动时没有对应的修复。概率很低，但一旦发生，两次 Clean Database 后元数据就丢了。可以在 relocate 开始时检查：记录的文件不存在、但 `destination_for(...)` 存在且 id 一致时，就地改正记录。
4. **测试覆盖**：这一轮两条阻断的修复都没有配回归测试。建议补两个：
   - `read_body` 收到截断流时返回的错误，`terminal_import_error` 判为 false。
   - 搬家时让 save 失败，并且原目录里只有这一个文件，检查文件和补丁都回到了原处。
5. **`/api/info.interface_binding.platform`**：这个接口对匿名访客开放，现在会把服务器的 OS 名称暴露出去。信息量很小，如果介意可以只返回两个 scope。

## 对 dsh 「需要你看的」两点

- **`browse_manifest`/`relay_urls` 快照**：同意可以不做。把措辞改成如实描述就够了，这两项不阻塞合并。
- **真机验收**：`share-followups-2026-11-08.md` I 节里的项目仍然要你来跑。建议优先跑「断网恢复」，这正好覆盖阻断 1 修改后的行为。
