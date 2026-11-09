# 共享功能未解决项备忘（2026-11-08）

来源：`session-ef016726` 的评审文档（`share-pr-review.md`，835 行，第 0–16 章）、该会话 357 条真实用户消息、
以及 2026-11-08 当天的代码核对。本文件是工作备忘，条目随对应 spec 落地后逐条勾掉；不是对外文档。

## 三项决定（用户，2026-11-08）

1. **端口默认随机 + 6–12 小时轮换**（已实施，见下）。
2. **255 字节那整条放弃**：新文件系统（020）会让它自然消失；已改的代码已全部回退（工作树干净）。
3. **99% 穿透、中继加入/退出归入 022**，先核实现状再写 spec。

## A. 用户指出过、但此前没做也没回

| 诉求 | 现状 | 归属 |
|---|---|---|
| 随机端口 + 6–12 小时轮换（25184、26696） | ✅ 本次实施：`share_udp_port` 默认 `""`（每次启动/轮换随机），`share_port_rotate` 默认 `480` 分钟（±25% 抖动，钳在 6–12 小时）；中继 TCP 端口保持固定 11205（反代要指向它）；`share_settings.rs` 默认值测试、`guide/sharing.md` 中英两版、三语设置提示同步 | 已完成（本次） |
| 中继节点自由加入/退出（24046） | 部分：允许名单已接受节点码（`parse_allow_entry`），协助离开已发 `HelperLeave`；销毁节点不切断中继、删除导入/进程退出不发离开 | 022 |
| 内网穿透覆盖率 ≥99%（28939） | 机制齐全（直连、打洞、UPnP/PCP/NAT-PMP、手动外部地址、自建/公共中继、pkarr、mDNS），`scripts/share-network-matrix.sh` 15 种拓扑；**没有真实网络下的度量** | 022 |
| 冲突文件在 temp 存多久（43210） | 24 小时（`PENDING_RENAME_MAX_AGE`）；020 会删掉整套机制 | 020 |
| 所有 zip 都绕开橙云（41150） | 设置 `direct_download_url` 后才绕开；未设置时全在页面域 | 021 |
| 下载插件走网络接口、一键切换（30651） | 只有设置项，没有一键切换 | 022 |
| Deno 100% 兼容（31963、31988、39182） | 13.1.2 修订的六条风险全在（shim 无条件加载最严重） | 022 |
| 共享页图片压缩（24563、25184） | 分享页 page 走 `archives::serve_page`，应已生效，未验证 | 022 验收 |
| 分享率只计已校验字节（26451） | 已实现（`share_net/server.rs:593` 注释），未给证据 | 022 验收 |
| API 是否全走 casbin、能否用 token（23262） | 无结论（曾被擅自升格为 023，已撤销） | 待定，不立 spec |
| 「没做完的任务你都做就行了」（42399） | 评审 1–13 章未做项此前一条没列 | 本文件 C 节 |

## B. 已改、但用户没验收

- 重连：`Run` 不再持旧 `net`、session 改 `RwLock`、重连清 `tried_helpers`、`flush_ratios`、600 秒窗口（超过 600 秒仍是终态失败）。
- 接口：接口不存在时 `bind()` 报错不退回全部网卡；15 秒 `spawn_interface_watch`，比对实际绑定地址，IP 变即重绑。
- 预选接口只校验格式；`spawn_record_refresh` 用 `Weak` + `Arc::ptr_eq`；`retract_node_record` 每个中继重试 3 次。
- 下载：单行本/补丁 zip 边打包边发；`serve_archive_download` 强 ETag + `If-Range`；未生成完的 Range 返 503 + `Retry-After: 5`（临时文件 TTL/清理未核实完整）。
- `share_external_addrs` 恢复、`TransferGuard` 覆盖 `write_all`、`clear_ip_transports()`。
- 当天提交：`b1723b6`（入库未完成不可见）、双域名直连、右键分类子菜单、滚动条不复发。

## C. 评审 1–13 章仍未做（按 spec 归口）

**022 share-network-hardening**
- 1.4.2 fork `netwatch`/`iroh-relay` 打 `bind_device`（`[patch.crates-io]`）。
- `KnockV1` 与旧记录兼容；销毁节点即切断中继；中继与端点重启解耦；轮换等活跃 claim；地址变化立即重发记录；
  网络错误统一转「等待网络」；下载空闲超时 + 自动重试。
- 13.1 插件出口六条：shim 无条件加载、`restart_workers` 杀插件、绑定失败静默回退、不拦 `sendmsg`、
  glibc DNS 泄漏、FreeBSD/macOS 不支持；13.1.3 FreeBSD FIB。
- 13.2 预热代数、每页 `list_pages`、预热不受 `LoadThrottle`；13.3 `enableresize` 默认值；
  13.5 `opt-level`；13.6 两个空文件 `=13.4`、`=535,driver`。
- 99% 穿透的可度量验收；中继加入/退出完整化。

**021 direct-download-domain**
- 15.2-3 下载地址 `http://` 校验（`settings.rs:621` 仍只查 scheme 形状）。
- 15.2-7 nginx 样本；`sharing.md` 里「几 GB 归档经 CDN 等不到首字节」的过时说法；`docs/dist` 重建。

**020 hash-named-library**
- 全部；含删除冲突/重命名状态机与 `PUT /archives/{id}/rename`。

**023 share-crate-extraction**
- 共享拆独立 crate（`share_net/` 约 7,100 行 + 4 个文件约 10,700 行）。

## D. 核实记录：99% 穿透与中继加入/退出

**穿透机制（代码在）**
- 局域网直连、iroh 打洞（两侧 cone NAT 也能通）、`share_portmap` 的 UPnP/PCP/NAT-PMP、
  `share_external_addrs` 手动地址、自建中继（proxy/acme/files 三种 TLS 模式）、公共中继、
  `share_nodes` + pkarr rendezvous、隔离局域网 mDNS。
- `scripts/share-network-matrix.sh` 覆盖 15 种：1:1 NAT + 外部地址、无 UPnP 打洞、双侧 CGNAT 纯中继、
  UDP 被封走 TCP 443、HTTP 代理、SOCKS5、IPv6-only、容器 DNAT、TLS 检查代理、隔离 LAN mDNS、
  WireGuard 直连、UPnP 路由器、frp UDP、自建中继 proxy + frp、导入方环境代理。
- 缺的是**度量**：没有真实互联网上的成功率统计，也没有「失败时怎么办」的验收标准。

**中继加入/退出**
- 允许名单：`share_node_relay_allow` 现在接受节点码（`lrrg-node:…`）或端点 id（`nodes.rs:605-616`）。
- 协助离开：关闭协助时发 `HelperLeave`（`share_imports.rs:652`）；删除导入、进程退出、端点停止不发，
  对方最多保留 15 分钟；`leave()` 失败不重试。
- 销毁节点只换节点码不换端点身份，知道端点/中继地址的人仍能连；旧码作废记录只 PUT 一次（现已重试 3 次）。

## D2. 020 已定的规则（用户，2026-11-08）

- 新入库一律 `{id}.{ext}`，2 个十六进制字符扇出（256 桶），sidecar 是同桶的 `{id}.json`。
- `_byname` 里保持原名与原始相对路径，**不改名、不写 sidecar**，按文件名识别。名字用 `_byname` 而不是
  `.classic`（2026-11-08 定）：它按名字寻址而非按 id，且前导下划线在资源管理器/Finder 里排在桶目录之前、
  又是可见目录（点目录会被 Windows/macOS 默认隐藏）。
- 用户挂载目录里凡是「原始文件名 + 子目录」管理的档案，扫描时**一律自动移动**到
  `_byname/<原始相对路径>`；新丢进去的文件同样处理。没有预览/确认/回滚作业，移动是纯 rename，
  记录里保留 `original_path`，需要时手工移回即可。

> 这三条已写进 020 的 spec/plan/research/data-model/contracts/quickstart（含把原来的 migration job 与
> `/api/migration*` 端点整段删掉）。

## F. 旧会话里的 UI 问题（357 条真实用户消息扫描，状态未逐条核实）

来源：`session-ef016726` 导出的 `/tmp/um3.txt`，括号内是 JSONL 行号。标注「已修」的是当天确认过的，其余需要
用户指认或界面复核——纯视觉问题 agent 无法代替用户目视验收。

- 向导：内容偏左、右边距过大（4473、4747）；版本历史在移动端看不到 select（4256）。
- 主页：batch operations 按钮缺 i18n、位置偏下（8352）；底部 GitHub/主题/版本/语言排版乱（9255）。
- 文档站：主题切换按钮容器变形（9536）。
- 队列：超过 25 项才允许滚动（6983）；队列组滚动条改窄（12304）。
- 阅读器：首次打开超大余白（7123）；移动端图片 placeholder 缩在右中央成小点（24912、33835）；
  4100 替换时仍整张闪烁（36059）。
- 搜索：结果理由要 icon + tooltip（9936）；最近搜索文字叠到 input 左上角（10210）；token 续期导致背景黑/黄闪
  （10330、10480、10578）；无推荐时排版（10977）。
- 智能创建单行本：drawer 丑、分页限一行（11378）；input 与 `/26` 高度不齐（11836）；箭头/按钮圆角与基线
  （11837、12028、12176）；边框灰色生硬（11904）；带 ✅ 图标高度不一致（11986）。
- 移动端「创建/添加到单行本」按钮宽度与翻页组件挤压（12100–13221，多轮返工）；候选区高度不一 + 批量处理选中
  项为空（13723）；等高后闪烁复发（14055、14066）。
- 表单：drawer 的 number stepper 颜色、未用 common-ui（14133、14152）；datepicker 文字颜色（15006、15087、15300）。
- 中继/连接：modal 改 switch（15567）；缺生成/复制节点码（16039、16040）；按钮位置尺寸乱（16320）；tooltip 需换行
  （16403）；CheckBox 是否 common-ui（19485）；自己的节点与添加节点要区分、输入框改 input（19864）。
- 手机点网络接口 select 导致 modal 偏移（20096）；销毁重建按钮同排/改图标/tooltip（20098–20100）；接口说明应改完
  自动重启（20529）；四个设置按钮改图标、VPN 数据填哪（21243）；分享率快捷值 1.0/1.1/5.0/10.0（21396、21473）；
  LANrurugi 节点左对齐（21823）。
- 中继自由加入退出的 UI（24046）；两个表格 UI 统一（29017）；「验证并添加」（29179）；下载插件一键切 VPN 的 UI
  （30651）；下载出口 UI 说明不足（31631）；保存设置按钮默认显示（33186）；中继删除按钮 + modal（37884）。
- 右键菜单分类子菜单展不开（43086）**已修**（`91d5464`）；菜单滚动条不要复发（43369、43404）**已修**（`e486ab3`
  与 `91d5464`）。

## E. 下一步

1. 本备忘随 020–023 的 spec 落地逐条消化。
2. 022 先把 99% 与中继加入/退出写成可验收条目，再谈实现。

## G. 进度快照（2026-11-09 01:10，收工）

分支 `feat/share-bind-interface`（PR #258），工作区干净，已推送至 `3cf14c2`。

**已提交**
- `6a7bb7e` 020 文档定稿为 `_byname` 布局。
- `3211b5f` CI 修复：`.cargo/config.toml` 的 `CFLAGS` 让 GCC 15 能编 libarchive2-sys 自带的 libarchive。
- `4afbca0` 020–023 的 `tasks.md`。
- `9116e91` 021：`direct_download_url` 校验（scheme/凭据/query）、保存时告警、双域名 nginx 参考配置、中英文档。
- `51998a6` 020 Phase A：新入库落 `<ab>/{id}.{ext}` + `{id}.json` sidecar、标题由文件名清洗、扫描末尾把其余档案搬进 `_byname/`、删 `PUT /api/archives/{id}/rename` 与编辑页重命名。
- `3cf14c2` 022 的一部分：下载流的 180 秒空闲超时、连接超时与 TCP keepalive。

**已知未做（下次接着来）**
1. 020：冲突机制仍休眠未删（`stage_pending_rename`/resolve/compare 端点、队列里的冲突菜单、`FilenameTemplateEditor.tsx`）；rebuild 的差异报告与清库重建测试；`POST /api/scan` 暴露 `relocated`/`skipped`/`failed` 计数；quickstart S1–S10 实测。
2. 022：除 `T031` 外全部未做——尤其是 fork `netwatch`/`iroh-relay` 打 `SO_BINDTODEVICE`（US1 的核心，工作量大、要在真实 VPN 环境验证）、`waiting_for_network` 替代终态、轮换按 claim 推迟、销毁即撤销、中继与端点解耦、`HelperLeave`、repack 缓存重写、99% 测量。
3. 023：只有 spec/plan/research/data-model/contracts/quickstart/tasks，一行代码未动；它必须排在 022 之后。
4. 021 剩余：quickstart A/B（真实 5 GB 下载、无 cookie 分享下载）需要在重建后的 dev 容器里实测。
5. 备忘 F 节的 UI 清单仍未逐条核实。

**下次开工建议顺序**：020 的休眠冲突代码清理（机械、可单测）→ 022 US6 的 repack 缓存与下载自动重试 → 022 US1 的 fork（单独一个 PR，需真机 VPN）→ 023。

## H. 2026-11-09 01:35 全量测试的 OOM

`setsid mise run test`（`--workspace --no-fail-fast --lib --bins --tests`）在编译
`lanrurugi-book-worker`（pdfium-render + cosmic-text，`CARGO_BUILD_JOBS=1`）时被 SIGKILL（exit 137）
—— 与 CLAUDE.md 记录的 `memory.high` 命中方式一致，不是测试失败。本次已跑的 crate 级门禁
（storage 129、scanner 120、api 676+1+1+5）都通过；全量套件留待调高 `memory.high`
（或留到 CI，CI 上的 Ubuntu runner 内存充足）后再跑，不作为 020 的阻塞项。

## I. 验收清单（2026-11-09 09:45，给真机/真环境用的）

代码侧能自动验的都已经在 CI 里（fmt/clippy/check-crate/单测/docs-api --check/tsc/eslint）；
下面这些是「只有你知道对不对」的部分，按 020 → 021 → 022 的顺序走最快。

### 020 哈希命名与 `_byname`

1. 上传一个真 zip（任意大小），确认入库后磁盘上是 `<library>/<前两位>/{id}.zip`，同目录有 `{id}.json`；
   Redis 记录 `name` 是原文件名去扩展名、`title` 是去掉 `[DL版]`/`(中国翻訳)` 之类之后的标题。
2. 下载该档案，确认保存的名字是**标题**（不是 40 位 id）。
3. 同一个文件再上传一次：应被拒绝（内容重复），并报出已有档案的 id，不产生第二份字节。
4. 往库里丢一个新文件（SMB/rsync/`cp` 都行），等 60 秒后触发扫描：它应先被收录，再被搬进
   `_byname/<原相对路径>/<原名>`；原目录里那份消失；再次扫描不应重复收录。
5. 手动把一个档案从 `_byname/` 移回库根：下一次扫描应把它搬回 `_byname/`（幂等）。
6. 目标位置已存在同名但内容不同的文件时，扫描应跳过并计数，**不覆盖**。
7. 编辑档案页不应再有「文件名 + 重命名」那一行；`PUT /api/archives/{id}/rename` 应 404。
8. 升级后第一次扫描的日志里应有 `_byname relocation pass` 与搬家数量；重建索引的结果 JSON 里应有
   `relocated`/`relocation_skipped`/`relocation_failed` 与 `sidecars` 审计块。

### 021 双域名下载

9. 设置里把「档案下载地址」填成灰云子域（如 `https://fs.example.org`）；页面地址是 https 时，
   填 `http://` 应被拒绝（400），填带账号密码或 `?query` 的地址也应被拒绝。
10. 用一个**未登录**的浏览器打开分享页，点下载：应能下到文件（不需要 cookie、不需要登录），
    文件名是标题。
11. `curl -I` 该下载链接应看到 `accept-ranges: bytes` 与 `etag`；`curl -r 0-1023` 应返回 206。
12. 用 `Host: fs.example.org` 直接请求主站路径（如 `/api/archives/...`）应 404；请求 `/api/dl/...`
    才通。反向代理按 `docs/public/nginx/lanrurugi-dual-domain.conf` 配好后，
    `nginx -t` 应通过、两段 server 的 `allow` 只放 Cloudflare 网段。
13. 在橙云开着的情况下下载一个 >1 GB 的单行本：不应出现 524（首字节应立即到达）。

### 022 VPN 固定与恢复（需要真的 tun0）

14. 设置里把「分享网络接口」和「插件网络接口」都固定到 `tun0`，然后 `tcpdump -i <物理网卡>` 抓包，
    做一次分享导入 + 一次插件下载：物理网卡上不应出现对端/目标站的流量（mDNS/局域网发现除外）。
15. 断开并重连 VPN（拿到新 IP）：分享端点应自动重建，状态页显示新的绑定地址，不需要手动保存；
    期间进行中的导入应自己继续，而不是变成终态失败。
16. 把 VPN 接口停掉（`ip link set tun0 down`）：设置页应显示「接口不存在」，而不是退回默认路由继续发。
17. 端口轮换开着、导入正在进行时触发一次轮换：导入不应断开；日志应有
    `port rotation deferred`。
18. 「销毁并重建节点」后：旧的节点码解析不到任何端点（不是解析到一个还能连的地址）。
19. 中继角色的允许名单里加一个 `lrrg-node:` 条目后，只有该节点能经中继转发；把中继端口改掉只重启中继，
    不重建端点（日志应有 `kept the running relay across the endpoint rebuild`）。
20. 断网 30 秒再恢复：导入应自己继续（`waiting_for_network` 计数会先涨后停），不需要点重试。

### 023 共享引擎拆分

21. 现状是**部分实现**（crate 与 `ShareHost` 已就位，`nodes`/`publish`/`helper`/`server`/`import` 仍在 api）。
    验收方式只有一条：行为与拆分前完全一致——上面 14–20 全部通过即可，不需要额外步骤。

## J. 收尾快照（2026-11-09 10:50）

分支 `feat/share-bind-interface`，HEAD `808dbe5`（本轮再加一个测试 commit），CI 全绿。

**020 剩下 0 个代码任务**：29 项勾选 + T014 在本轮补齐（600 字节标题、重复字节只入一次、
全是发行标记的名字回退成文件名、删掉的端点 404）。真机项在 I 节。
**021 剩下 0 个代码任务**：13 项勾选 + T014（文档无相关承诺，不需改）；T012/T013/T013a
是真实下载与无 cookie 分享下载，见 I 节。
**022**：26 项完成，14 项全部是真机/跨平台验证（抓包、FreeBSD `setfib`、macOS 拒绝、
99% 基线、quickstart C/E/F/G/H），见 I 节；中继访问与 `HelperLeave` 的剩余触发条件
已经写在 T020/T029 里，没有假装做完。
**023**：crate 与 `ShareHost` 就位、六个模块已搬，`nodes/publish/helper/server/import`
约 5,100 行仍在 api，原因写在 `specs/023-share-crate-extraction/tasks.md` 的
Implementation Record 里。

给 Claude Code 的 review 提示：`specs/review-prompt-2026-11-09.md`（整段贴过去即可）。
