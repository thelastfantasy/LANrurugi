简体中文 | [English](./README.en.md) | [日本語](./README.ja.md)

# LANrurugi

[LANraragi](https://github.com/Difegue/LANraragi)（一个自托管的漫画/同人志库管理器）的从零开始的
Rust + React 重写版本。目标是与现有 LANraragi 部署实现功能对等，并保持完整的数据/API 兼容性，同时修复一个
已知的重复检测缺陷，并在旧版 Perl 实现完全没有的地方加入真正的多核并发能力。

**状态**：Phase 1（`specs/001-lanrurugi-full-rewrite/`）已实现——全部八个用户故事（库连续性、非合并式
入库、第三方 API 兼容性、插件元数据增强、备份/导出、重复项修复、UI 本地化，以及一项对比旧系统的并发基准
测试）均已完成；完整拆解见 `specs/001-lanrurugi-full-rewrite/tasks.md`。四个附加于 Phase 1 之上的补充功能
也已全部实现：`specs/002-job-console/`（后台任务管理 UI）、`specs/003-ui-test-automation/`（Vitest +
Playwright 前端测试覆盖）、`specs/005-download-plugin-progress/`（真实的字节级下载进度、按域名的并发/
限速），以及 `specs/007-guest-restricted-access/`（受限访客访问模式，取代旧版"全开/全锁"式密码开关）——
后者的完整 `specs/007-guest-restricted-access/quickstart.md` 11 项场景真机验证仍待跑一遍
（`tasks.md` T053），代码本身已完成并通过全部自动化测试。Phase 2（`specs/004-ocr-manga-translation/`，页面内漫画翻译）代码也已实现并接入阅读器：检测/识别、
翻译后端、本卷字体缓存、擦除与排版渲染均已可用；其 `quickstart.md` 的五项真机场景仍建议在准备真实
OCR 模型文件后逐一复核。本轮多条工作线（OCR/擦除/嵌入模型进程隔离、登录设备管理与跨域登录交接、
稳定设备身份、原始档案搜索，以及配套前端与构建收尾）逐项记录见下方“近期实现增量”。


## 近期实现增量（2026-09-22 起）

> 本节按多轮真机调试上下文记录这一批实现与收尾改动：OCR 阅读方向与翻译候选、DenseCRF 字形掩膜擦除、
> 嵌入模型子进程隔离、双窗口刷新令牌/设备管理、跨域登录交接与稳定设备身份（2026-09-23 增补）、
> 原始档案搜索，以及配套前端与构建收尾。

### OCR 阅读方向 / 旋转候选 / LLM 二选一

- **方向字段进入持久化数据模型。** `DetectedTextRegion` 新增 `writing_direction: Option<WritingDirection>`
  与 `alternate_source_text: Option<String>`，两者 `#[serde(default, skip_serializing_if = "Option::is_none")]`，
  保证 Redis 中旧版 JSON 可继续反序列化；`WritingDirection` 为 `Vertical | Horizontal` 封闭枚举。
- **新增纯函数 `rotate_cw` 与投影方向判据 `reading_axis`。** 对检测框内墨迹做 x/y 投影，按“单个方向存在
  周期性窄条/间隔、另一方向没有同样强的多段结构”区分竖排、横排与 `Ambiguous`；含糊时不在原始识别路径里
  猜方向。
- **歧义方向串行补识别。** 主识别成功后，对 `Ambiguous` 且**不与其他检测框相邻/重叠**的孤立框，串行执行
  一次 `recognize(rotate_cw(crop))` 作为候选 B；候选 B 与 A 不同且仍像日文时写入
  `alternate_source_text`。串行执行是为了避免高负载时把 GPU worker 并发打崩；相邻框则留给下面的整块 union
  路径处理，避免把碎片逐个旋转识别。
- **检测碎片整块复识别。** 检测模型经常把一块竖排/多列文字拆成多个重叠框，逐框识别会把字符交错。新增
  后处理：当最终 region 由两个以上原始检测框构成时，对这些框的并集 crop 再识别一次，结果写入候选 B。
  page 15 的三个标签正是这样修复的：`娘士`/`ーッ娘スー母猫`/`エ、ルフ母娘` 最终分别选中
  `戦士母娘` / `スーツ母娘` / `エルフ母娘`。
- **合并后的方向/候选元数据贯通。** `DetectedLine` 携带方向和候选 B；`merge_lines` 按阅读顺序拼接候选 B，
  并采用保守方向投票——只有所有成员都有相同且非歧义的方向时才把方向写入最终 region，否则留 `None`
  交给合成层的兜底逻辑。
- **LLM 二选一与写回。** 翻译请求构造时，如果存在 `alternate_source_text`，user message 会把 A/B 两个
  OCR 候选及“先判断哪个更像合理日文，再只输出中文译文”的指令一起发给模型；结构化输出新增
  `selected_source_text`，模型回传后把选中的候选写回 `region.source_text`，再持久化到
  `LRR_TRANSLATION_TEXT_*`。漏字段/空字段时默认选候选 B，避免已知有问题的旧 A 读数继续固化。
- **合成方向以测量值为先。** `composite::is_vertical_bubble` 现在优先读取 `writing_direction`，框宽高比
  与目标文字脚本只作旧数据/真歧义时的兜底；原文竖排 → 译文竖排、原文横排 → 译文横排不再被“目标文字是否
  CJK”误判。
- **去重与碎片保护。** `translation_pipeline` 的重叠 region 去重现在会同时比较 A/B 候选、使用字符多重集
  包含判断，并把小写假名折叠后比较（如 `ッ`/`ツ`），因此 page 15 的 `ーッ` 碎片不会另起一块重复绘制；
  `LRR_TRANSLATION_REGIONS` 仍保留检测层候选，实际“翻译了哪一条”记录在 `LRR_TRANSLATION_TEXT_*`。
- **同一轮 OCR 稳健性加固。** 识别新增最差 token 置信度门槛（默认 0.5），低置信度解码走可复现的内容失败
  路径而不是缓存坏结果；检测异常大框比例阈值从 0.05 放宽到 0.08 以保留真实的大型标题/印章框；
  `recognize.rs` 继续清理多余标点分隔符（`エ、ルフ母娘` → `エルフ母娘`）。

### DenseCRF 字形掩膜与擦除

- **新增 `lanrurugi-inpaint::densecrf` 与 `permutohedral`。** 这是 `philkr/densecrf`
  （`pydensecrf` 的底层 C++ 实现）标量路径的逐行 Rust 移植，只保留
  `zyddnys/manga-image-translator::mask_refinement` 实际使用的 `DenseCRF2D`、Gaussian/Bilateral pairwise、
  Potts compatibility 与 mean-field inference，学习/梯度/SSE 路径明确不移植。
- **`densecrf_stroke_mask` 成为真正的细化路径。** `stroke_mask.rs` 先用连通域把粗掩膜拆成单个文字组件，
  再对每个组件做位置+颜色双边 DenseCRF 细化；细化后的组件若仍几乎覆盖整块 crop 则丢弃，避免“整块糊掉”；
  失败时回退到已有的 `local_background_stroke_mask`。这套路径的动机与 page 13 真机测量都写进了模块注释。
- **擦除不再只依赖粗掩膜。** 合成/擦除侧现在可以用细化掩膜判断“哪些是字形像素”，减少整块气泡填充误擦、
  也减少 DenseCRF prior 覆盖不足导致的原文残留；诊断任务 `mise run debug-densecrf-prior` 与
  `mise run diag-probmap` 保留为真机调参入口。

### 嵌入模型子进程隔离（embed worker）

- **新增 `lanrurugi-embed-ipc` + `lanrurugi-embed-worker` 两个 workspace crate。**
  `lanrurugi-server` 不再在自身进程里长期持有 `lanrurugi-recommend` 的 ONNX 嵌入 session；模型被移入独立
  `lanrurugi-embed-worker`，由 `lanrurugi-api::embed_worker_client::EmbedWorkerClient` 按需 spawn、通过
  tarpc/Unix socket 调用，并在空闲后回收。目标是修复真机上报的“无请求时 server RSS 仍约 732MB”的内存
  基线问题。
- **调用侧全部异步化、串行化。** `recommend.rs`、`recommend_precompute.rs`、`tankoubon_grouping.rs`
  的嵌入调用改为 `embed().await`，内部由 worker 的单 session 串行处理；启动时不再有“等待模型下载/加载”
  的 gate，首个真实请求自行支付冷启动成本，之后走已热 worker。
- **部署同步。** `Dockerfile.dev` 构建并复制 `lanrurugi-embed-worker`，`Dockerfile.build` 增加
  `fonts-noto-cjk` 以满足中日文字体渲染/测试环境；`main.rs` 只负责创建 `RecommendService` 并启动
  worker 空闲回收任务。

### 双窗口刷新令牌 / 登录设备管理

- **有状态 refresh token 落地。** `lanrurugi-storage::refresh_tokens` 新增完整仓库：只存
  `sha256(secret)`，记录 `family_id`、`expires_at`（绝对上限）与 `idle_expires_at`（滑动空闲窗口），
  使用 Redis `WATCH`/`MULTI` 乐观锁完成轮换，旧 token 复用检测会一次性烧掉整个 family。
- **OAuth 2.1 风格轮换体验。** 新增 5 秒 reuse grace 与最多 3 次同 family 并发宽容，避免多标签页同时
  刷新误伤；grace 不延长绝对过期。默认绝对 7 天、空闲 14 天可由设置调整；默认最多 5 台登录设备，
  超过时登录侧淘汰最久未活跃的 family。
- **设备信息与审计。** `device_info.rs` 汇总 `woothee` UA 解析、MaxMind GeoLite2 城市信息与客户端
  Client Hints/屏幕/内存等表单字段；登录、刷新、设备改名/撤销都会写入结构化活动记录。
- **`/api/sessions` 管理界面。** 新增 `sessions.rs` 路由（Session cookie only，Admin/Guest Token 均
  明确 deny）：列出活跃登录 family、当前设备标记、自定义命名、撤销；设置页 `SecuritySection` 增加
  “活跃设备”列表、空闲/绝对刷新窗口、最大登录设备数等输入项。
- **认证边界同步收紧。** `route_policy.csv`/`authz.rs` 为 `/api/sessions` 加 Session-only 规则；
  `AuthContext` 增加 `session_family_id`，活动记录可关联到产生它的登录 family；服务端测试补充
  auth_flow / contract_api 覆盖。

### 跨域登录交接 / 稳定设备身份（2026-09-23 增补）

- **可信等价域名之间的自动登录交接。** 设置页新增“可信等价域名”（每行一个精确 origin）；
  `localhost` / `127.0.0.1` / `::1`（同 scheme/port）内置互通，无需填写。任一已登录域名都可以为
  其他域名提供一次性 `state` + `code` 交接，回调在目标域名落自己的 access/refresh cookie，不需要
  中心认证地址。可选“Cookie 共享域”（`.example.com`）用于同一注册域的子域，不同注册域走交接；
  关闭“自动发起跨域登录”则回退到本地登录表单。
- **稳定的设备身份 cookie。** 登录/刷新/跨域交接都会下发一年期 HttpOnly 的 `lanrurugi_device_id`
  （非凭据）。自定义设备名按设备身份（而非单次登录 family）索引，同一浏览器重新登录——包括
  localhost 与 127.0.0.1 互换——会继承旧名字并替换旧 family，不再显示成新设备。
- **设备上限即时生效。** 修改“最大登录设备数”后立即按最久未活跃淘汰多余 family，不必等下一次登录。

### 搜索 / API 增量

- **新增 `GET /api/search/archives`。** 复用 `/search` 的查询语法、过滤、排序与分页，但**不**把
  Tankoubon 成员折叠成 `TANK_` 聚合；每个结果额外返回 `tankid`、`archive_index`、`tank_sequence`
  与嵌套的 `tankoubon` 元数据，便于重复检测/入库去重按原始档案检索。Tankoubon 归属走反向索引，
  不使用全库扫描；反向索引读取失败时保留档案本体、降级为无 Tankoubon 元数据。

### 前端与交互收尾

- **安全设置页接入活跃设备管理。** 改密码输入框统一为公共 `Input`；新增刷新令牌空闲窗口、绝对窗口、
  最大登录设备数设置，以及活跃设备列表的改名/撤销/当前设备标记/toast/确认弹窗。
- **触摸设备评分清除。** `RatingWidget` 在无 hover 设备上显示不小于 24px 的垃圾桶按钮，保留 hover
  设备原有的右键清除与星级交互。
- **活动记录与 i18n。** `ActivityDetailPanel`/`activityTarget` 接入 session 设备事件；三套 locale
  补齐设备管理、刷新窗口、评分清除等文案；`index.css` 增加设备行操作样式。

### 构建、工具链与验证

- `.mise.toml` 增加 `debug-densecrf-prior` 与 `diag-probmap` 两个诊断任务；`Dockerfile.dev` 构建
  server + gpu worker + embed worker；`Dockerfile.build` 增加 Noto CJK 字体。
- 本轮验收命令：`mise run fmt`、`mise run check-crate -- lanrurugi-ocr lanrurugi-translate lanrurugi-api`、
  `mise run clippy`、`mise run test -- -p lanrurugi-translate --lib` 均通过；workspace 测试中
  `lanrurugi-ocr` 95、`lanrurugi-api` 275、`lanrurugi-storage` 82、`lanrurugi-translate` 158 全部通过。
- page 15 验证：三条标签的 `source_text` 最终为 `戦士母娘` / `スーツ母娘` / `エルフ母娘`，其中第一条
  按横排合成，后两条按原竖排方向合成；page 13 验证：整块擦除后蓝色/粉色手写像素基本与原图一致
  （蓝 1335 vs 原 1331，粉 3102 vs 原 3009），无彩色墨迹回退。
- 测试环境注意：`.env.test.local` 默认把 `LANRURUGI_TEST_REDIS_URL` 指向
  `redis://host.docker.internal:16380`；若该 Redis 未运行，`lanrurugi-storage` 会出现一个
  `bootstrap::...` 失败（82 个测试中的 1 个），这与代码无关。启动对应 Redis 后 82/82 通过。

## 技术栈

- **后端**：Rust（Tokio 异步运行时、Axum Web 框架、Rayon 用于 CPU 密集型并行计算），一个位于
  `crates/` 下的 Cargo workspace，产出单一二进制文件 `lanrurugi-server`，带有
  `serve` / `rebuild-index` / `bench` 三个子命令。
- **数据存储**：Redis，原样复用自旧版部署（五个逻辑数据库——见
  `crates/lanrurugi-storage/src/redis.rs`）。
- **前端**：React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query，位于
  `apps/frontend/` 下。
- **插件**：沙箱化的 Deno 子进程，每个插件命名空间一个（`crates/lanrurugi-plugin/`）。
- **页面内翻译（Phase 2）**：同一 workspace 下的 `crates/lanrurugi-{ocr,fontcache,translate}` 与
  `apps/frontend/src/translation/`，不是独立部署单元——OCR 检测用 `oar-ocr`、识别用 `ort` +
  `manga-ocr`（均为 Apache-2.0，仅 CPU），翻译适配 OpenAI 兼容 / Anthropic / DeepSeek 三种后端。
- **基准测试工具**：`crates/lanrurugi-bench/`——一个合成库生成器、`criterion` 微基准测试，以及一个同时驱动
  本二进制文件和一个真实旧版 LANraragi 实例进行并排对比的跨系统对比测试工具。

## 相较 LANraragi 的改进

### 新增功能

- **真实字节级下载进度与按域名限速。** 下载传输从插件沙箱移至 Rust（流式 reqwest），`downloaded_bytes`/`total_bytes` 实时流入进度条；支持按域名配置并发上限和令牌桶限速，设置项直接展开在插件卡片下方。
- **Tankoubon 连续阅读。** 所有成员档案页面按库内顺序拼接为连续流，翻页无缝跨越成员边界；阅读进度、书签、图章、ToC 等全部正确解析到对应成员档案的本地页码；ToC 优先显示 AI 命名的自定义章节名，未设置时回退到档案标题。
- **档案概览无限滚动。** 页面缩略图按需分批加载而非一次性全部渲染，长篇（尤其多卷 Tankoubon）打开概览不再卡顿；占位符预留真实尺寸避免滚动条跳动，回到顶部按钮重新定位而非级联加载跳过的页面。
- **更强大的漫画章节（ToC）划分。** 删除/编辑列出档案内全部章节供选择，而非旧版那种只能操作"阅读器当前滚动到哪一章"的限制；封面/封底/目录/彩页/omake/后记/插画等常见章节类型一键预设，或用数字键快捷设置（0 = 目录，1–9 = 对应章节）；预设写入的标题存为内部保留标识符并在重复设置时去重移位（而不是留下过期重复项），前端再映射回本地化显示文本。
- **图章与书签联动。** 在尚未添加书签的页面放置图章时自动为该页添加书签；移除该页最后一个图章时自动撤销
  该书签；两者均可在设置页独立关闭。联动动作在活动记录中合并为一条记录，不会与手动图章操作混淆；书签页
  悬浮预览网格上叠加显示每页图章数量徽章。
- **图章选框标注。** 在单点图章之外支持拖拽矩形选区，可配置颜色/填充/边角/显示方式，8 向手柄缩放、方向键微调、Ctrl+拖拽复制。
- **后台任务管理控制台。** 备份/恢复、缩略图重生成、重复扫描、索引重建等任务的可浏览管理页面，实时状态/进度/结果/过滤。
- **自动化前端测试套件。** Vitest + React Testing Library（单元/逻辑）+ Playwright（Chromium + Firefox 端到端），覆盖所有归档格式和历史已修复 bug 的回归测试。
- **可视化补页。** 上传时遇到内容高度重合的已有档案，自动像素级比对两个压缩包，智能识别相似页面与差异页并以并排预览呈现对齐结果；确认后进入双行缩略图排列界面，通过拖拽将多出的页面插入目标行任意位置，松开即生成精确的页面级补丁文件，无需手动定位或重命名。
- **页码级书签系统取代旧版分类关联式书签。** 旧版书签本质是一个全局开关，把档案整体加入某个指定分类；现在每一页都可独立标记书签，独立 `/bookmarks` 页面按最近变动/标题/入库时间三种方式排序、游标分页加载，卡片悬浮（触屏改为点击）展开该档案全部书签页的缩略图网格预览，支持删除单个书签、四角自适应屏幕空间定位；首页展板新增"书签"模式复用同一套预览交互，鼠标滚轮在预览网格与展板之间智能联动（网格内容可滚动时先滚网格，触底/触顶后短暂延迟防误触再转发横向滚动展板，横向滚轮鼠标直接跳过网格滚展板）；活动记录同步展示书签的增删操作。
- **大量 UI 改进。** 阅读页整页放大预览（lightbox）+ 快速滚动画廊；阅读页新增 Google Reader 风格 `j`/`k` 快捷键（`j` 向下滚动、触底自动翻页，`k` 回到上一页顶部），滚动距离可在设置面板按百分比或像素配置；移动端阅读页修复：锁屏后标签页被系统回收重建时不再被过期的 `?p=` URL 参数覆盖真实阅读进度、悬浮工具栏加背景色避免与下方内容重叠、帮助图标触屏点击不再同时弹出预览气泡和完整面板；分类下拉旁快速新建分类入口；Library 网格"标记为已读/未读"右键菜单；搜索语法新增空格作为 AND 分隔符、新增 `rating` 比较语法（`rating>=1`/`rating<4`/`rating=5` 等）、带引号的搜索值支持 `\"`/`\\` 转义；服务端注入当前主题消除首屏闪烁；集中化路由管理修复多处死链接；`date_added` 按日历日搜索，Library 默认按入库时间倒序排列；标签编辑器重建为芯片式输入；搜索过滤器支持浏览器前进/后退；分页组件当前页背景高亮且禁用重复点击；评分标签、Tankoubon 评分清除等若干显示与交互修复。
- **批量操作页支持批量删除。** 新增专用 `DELETE /api/archives` 接口（仅限 Session 登录调用，API Token 一律拒绝），删除前要求输入"删除"/`DELETE` 二次确认，部分失败时 toast 展示每个失败项的具体原因。
- **JWT 短期访问令牌 + 可轮换刷新令牌取代固定 24 小时 session。** 登录态基于真正的 JWT（HS256）access token（短期）与可撤销、单次使用、重放即整链失效的 refresh token（OAuth 2.1 风格轮换 + 复用检测）配合工作，前端在 access token 过期时透明续期，无需用户重新登录；两者有效期均可在设置页调整。
- **first-party API Token 管理系统取代旧版共享 apikey。** 旧版"整个实例共用一个固定 apikey 字符串"的机制被替换为可在设置页创建/重命名/撤销的多 Token 系统，每个 Token 可选 Admin（除 Token 管理/账号安全/数据库清空外的完整权限）或 Guest（只读）角色与到期时间，创建时一次性显示原始 Token 供复制；这是一项刻意的发布前破坏性变更，依赖旧版 `Bearer base64(apikey)` / `?key=` 机制的第三方客户端不再保证免修改可用（详见下方"刻意不作为改进"章节旁注）。
- **操作活动记录（审计日志）。** 结构化、持久化、可按操作者/操作类型/时间范围过滤的操作记录页面，区分手动操作（Session/Token 发起）与自动操作（扫描器入库、元数据插件自动运行），评分变更独立分类并展示新旧星级对比，设置页/插件页各分区支持深链接精准跳转到具体改动位置。可见性/删除权限见下方 Casbin 权限模型条目。
- **基于 Casbin 的声明式权限模型取代散落各处的硬编码判断。** 路由级"是否需要真实 Session 登录"（Token 管理、账号安全、数据库清空等）与 activity 记录的可见性范围，统一收敛为两份随代码走、经代码审查的 policy 文件（而非运行时可改的配置），替代旧版散落在各 handler 里的零散 `if is_token()`/`is_guest_token()` 判断。activity 记录的具体可见性规则：Session 可见全部记录并可删除；Admin Token 可见自身与全部 Guest Token 的记录（不含 Session 与其他 Admin Token）；Guest Token 仅可见自身记录；删除操作仅限 Session。
- **浏览器可自动发现的动态 OpenSearch 搜索。** `GET /opensearch.xml`（免登录，`Cache-Control: no-store`）按实际访问的域名/`X-Forwarded-Host`/`X-Forwarded-Proto` 动态生成搜索 URL 模板，内容随客户端 IP 变化；浏览器地址栏/搜索栏发起的搜索直接落到 Library 页的既有 `?q=` 搜索状态，无需额外点击。
- **受限访客访问模式取代旧版"全开/全锁"式密码开关。** 旧版 `enablepass`/`nofunmode` 只能产出"完全开放"或
  "完全锁死"两种状态，中间没有任何过渡；现在密码保护始终强制开启、不可关闭，取而代之的是一个可选的
  "访客模式"总开关 + 逐分类的"对访客可见"标记：管理员开启访客模式后，把想公开的分类逐个标记为访客可见，
  未登录访问者即被路由进一个受限的只读浏览体验（列表、阅读、搜索/标签过滤，范围仅限访客可见分类），而不是
  被重定向到登录页；书签、进度保存、原始文件下载、全部管理功能对访客一律不可用。范围外的档案返回 404（与
  不存在的档案完全无法区分），而不是 403，避免泄露"这个 ID 存在，只是你看不了"的信息。
- **真 3D 标签云取代旧版 2D jQCloud。** 词云球体持续自转、可拖拽/悬停交互，按权重十档渐变着色（五套主题各自定义色带，而非旧版仅有的四色分组）；点击球面上的词自动展开下方详细统计并滚动高亮对应行；标签数量较少时球体与外层容器同步按比例收缩，避免大库变小库后留下一片空球悬浮的空旷画面。
- **404/403 错误页面取代旧版空白页。** 未知路由不再显示无导航栏的空白主体，而是在完整 Layout 外壳（导航/主题/i18n/Footer）内展示明确的"页面不存在"提示与返回书库入口；阅读页打开不存在的档案/单行本 ID 时原地内联显示同一套 404 内容，URL 始终保持在访问时的地址（不跳转到独立错误页）；403 权限不足时区分未登录（跳转登录页）与已登录但权限不足（展示后端返回的具体拒绝原因）两种场景；应用渲染崩溃时展示通用错误页而非白屏。顺带修复了一个既有 bug：未登录访问任何页面都会被 Footer/更新横幅的无条件 `GET /info`/`GET /settings` 调用触发强制跳转登录页——即使是 legacy 本就允许匿名查看的页面（如统计页）也不例外；现已将 `/info` 与主题/语言字段迁移到未登录可访问的公开端点。

### 数据完整性与重复检测

- **修复了旧版的误判去重合并缺陷。** 旧版用文件前 512KB 的 SHA-1 作为 ID，两个共享前缀但长度不同的文件会碰撞并被静默合并。LANrurugi 将文件真实大小折入哈希输入，同时保留旧版 ID 以保证读兼容。
- **为已被该缺陷损坏的库提供一次性修复工具**（`lanrurugi rebuild-index` / `POST /database/rebuild-index`），重新计算每个归档 ID，曾被误合并隐藏的文件会重新出现为独立条目。
- **文件名冲突与内容冲突区分处理。** 内容重复无条件拒绝；文件名冲突提供覆盖/重命名两种选择，暂存已下载字节等待用户决定，超时自动清理。
- **消除下载完成时的重复编目竞态。** 下载路径和文件监视器原本可能同时编目同一文件导致数据损坏，通过跨 crate 共享的按文件名互斥锁修复。
- **pagecount/arcsize 自动自愈。** 每次启动自动扫描并修复 pagecount 为 0 的归档，损坏文件记录失败时间戳避免重复尝试。
- **单页图片损坏时故障隔离。** 解码失败的单页记录为已知损坏，阅读时返回占位图而非透传损坏字节。

### 并发与架构

- **单一进程（vs 旧版三个独立进程）。** HTTP API、文件监视器、插件池在同一 Tokio 运行时内以任务形式运行。
- **修复并发下载竞态条件。** 通过跨越"检查→写入"全窗口的按文件名异步锁，防止两个下载静默覆盖彼此的文件。
- **协作式下载取消。** 每个队列项持有 `CancellationToken`，在已有的网络错误检查点感知停止，复用部分文件清理路径。
- **明确、正确的 CPU/异步桥接。** 所有 CPU 密集工作通过 rayon + `spawn_blocking` 运行，批量扫描不阻塞 HTTP 请求。
- **请求合并。** 同一缺失缩略图或页面的并发请求合并为一次生成/读取。

### 下载流水线

- **非 ASCII 下载文件名修复。** 真实的 UTF-8 `Content-Disposition` 文件名不再因 `to_str()` 失败而退化为无意义的 ID 字符串。
- **下载取消是持久化状态。** 队列项状态机新增 `Cancelled` 状态，页面刷新后保留，有独立的 UI 处理。
- **重复在途下载拒绝（409）。** 同一 URL 的并发下载被拒绝而非静默允许；运行中的下载对应的队列记录不可删除。
- **服务重启后悬空队列项自动标记失败。** 启动时扫描队列，将因进程重启而丢失进度追踪的条目标记为可重试的错误状态。

### 错误处理与国际化

- **每个下载队列错误都是结构化且可翻译的。** `QueueError` 是零自由文本的封闭枚举，每个变体有稳定的数字代码，前端渲染为翻译后的字符串，重复归档错误中的 `existing_id` 会转为可点击链接。
- **插件 SDK 同样获得结构化错误处理。** 约 40 处 `throw new Error("...")` 被转为 `{error_code, data}`，`error_code` 同时作为 i18n 查找键。

### 插件沙箱化

- **每个插件的最小权限原则。** 每个命名空间拥有自己的 Deno 子进程，仅以该插件声明的确切网络/读/写权限启动，通过一次零权限启动探测查询权限。
- **插件命名空间参数的路径遍历加固。** 拒绝 `..` 和绝对路径，有专门测试覆盖。
- **一个插件的失败不拖垮其他插件。** 崩溃或超时的插件只丢弃该 worker，惰性重新生成。

### AI / LLM 功能

AI 不是零散玩具功能，而是把几个过去只能手工完成的场景做成了完整闭环。其中 **AI 插件创建向导** 是核心旗舰：它让“给某个网站写一个 LANraragi 插件”这件事，从需要阅读插件 SDK、手写正则和调试抓取逻辑，变成“用自然语言描述 → 自动生成 → 沙箱试运行 → 自动修复 → 确认安装 → 审计留痕”的一条龙流程。在 LANraragi 及同类自托管漫画服务器中，这种完整闭环目前极为少见。

#### AI 插件创建向导（旗舰功能）

- **自然语言直接生成可运行的登录/元数据/下载插件。** 输入目标域名后，系统先告诉你该域名已经有哪些插件类型被覆盖；对缺失的类型，你只需要描述页面特征、给出测试链接或测试凭据，AI 通过 DeepSeek 工具调用循环生成对应的 `.ts` 插件草稿。真正的页面抓取、试运行等网络访问全部由系统执行，AI 只负责语义判断。
- **生成后不是“看起来能用”，而是真实试运行。** 草稿复用 Deno 沙箱的两阶段权限模型，用你提供的测试链接/登录凭据真实执行一遍；试运行通过后还要你显式确认才会安装。手动编辑或让 AI 根据报错自动修复（最多连续 3 次，历史不丢弃）都会重新试运行，避免把坏插件装进库里。
- **试运行失败会自动判断是否登录问题。** 如果元数据/下载插件失败，AI 会判断是否与登录有关，并引导你就地生成或关联一个登录插件；成功后原草稿自动重新生成并关联，无需重开向导。
- **测试凭据永远不会发给 AI。** 登录调用只在本地执行，AI 只拿到脱敏后的结果——这是“让 AI 写插件”这个功能敢用于真实站点而不是只做 demo 的关键。
- **AI 生成插件有标记、可导出、可审计。** 已安装列表里一眼区分 AI 生成插件；任何插件都可一键导出 `.zip`；保存成功/失败都会写入活动记录，独立于手动上传。
- **为插件匹配设计了更合理的 `domain_match`。** 把“域名归属判断”和“真实触发匹配”拆开，避免插件路径写窄后向导或上传页“获取元数据”按钮漏判；未声明时自动兼容旧 `url_pattern` 行为。

#### 其他 AI 能力

除 AI 插件向导外，其余 AI/LLM 能力都可选，不配 DeepSeek Key 也能优雅降级（纯本地 embedding 或直接跳过），从不强制依赖：

- **统一 LLM 调用层。** 所有依赖 DeepSeek 的功能共用同一个 `LlmClient` trait：调用前统一检查 API Key，并向 DeepSeek 官方 `/user/balance` 校验账户余额（成功结果缓存 60 秒；自定义兼容端点不支持余额接口时放行）；提示词统一集中在 `crates/lanrurugi-api/src/llm_prompts.rs`。
- **智能推荐引擎。** 阅读到边界时展示推荐卡片（桌面 10 张/平板 6 张/手机 4 张），本地 ONNX 嵌入模型粗筛 + DeepSeek 大语言模型精排；Tankoubon 阅读边界以其最后一卷为锚点计算推荐，并排除同单行本其余卷。
- **AI Tankoubon 智能编辑。** 一键分析成员标题中的卷号/话数信息，自动建议单行本名称、章节标题和正确阅读顺序；支持整体命名和逐卷章节名智能建议，多候选翻页预览，即时应用。
- **AI 智能创建 Tankoubon。** 分析尚未加入任何 Tankoubon 的散档案，自动建议可能属于同一系列的分组，也会建议将散档案加入已存在的 Tankoubon；勾选确认后一键创建/添加，纯本地嵌入模型，不依赖 LLM Key。配置 LLM Key 后可选在创建时一并自动应用 AI 重命名与章节排序；不满意的建议可标记“不再提醒”。
- **LLM 标签自动回填。** 用 LLM 从标题中识别并补全缺失的作者/coser 标签，提升分组建议等功能的准确度。
- **压缩包拆包建议。** 自动检测多卷/多层结构或含嵌套压缩包的档案，用 LLM 生成拆包建议；支持递归目录树查看（嵌套子压缩包子子包也会展开）、存量档案一键生成拆包建议、单行本内批量拆包；阅读页可查看并手动执行后台拆包任务，支持 SSE 进度、同目录输出、Tankoubon 成员迁移和可选的删除原文件。
- **子目录批量创建单行本。** 在设置页有开关（默认开启）控制该功能，并在插件维护脚本中执行“子文件夹转单行本”，扫描漫画目录后只识别漫画目录下的一级子目录，为每个直接包含档案的一级子目录创建一个 Tankoubon，并把该一级目录下所有层级（含更深层嵌套目录）中的档案都加入这个单行本；嵌套目录本身不单独创建单行本。配置 DeepSeek Key 时可用 LLM 识别更合适的单行本名称和作者/社团标签，未配置时直接用目录名并跳过标签。

#### 页面内漫画翻译（Phase 2）

在阅读器中就地把漫画对白翻译成你的目标语言，整套 OCR → 翻译 → 排版渲染的流水线都在本项目内完成，
不依赖任何外部翻译服务的现成组件（`specs/004-ocr-manga-translation/`）。默认关闭，关闭时阅读路径
不增加任何额外开销。

- **专为漫画选型的 OCR。** 检测用 `oar-ocr`（PP-OCR，Apache-2.0），识别用自建的 `ort` 集成 +
  kha-white 的 Apache-2.0 `manga-ocr` 权重——通用 OCR 对竖排日文（tategaki）、拟声词和艺术字体
  识别率明显偏低，这是选择漫画专用识别模型而非通用模型的原因。CPU 执行器始终作为显式兜底可用；
  NVIDIA CUDA 可选启用（逐 session 独立尝试，失败或不可用时自动退回 CPU，互不影响）。OpenVINO/
  DirectML 仍不使用：注册不受支持的 OpenVINO 执行器曾被证实会在 `session.run()` 内无限挂起而非
  干净报错，这个结论只针对 OpenVINO，不适用于 CUDA（CUDA 不可用时会干净失败，可正常捕获降级）。
- **三段式本卷字体缓存。** 投票 → 锁定 → 熔断，避免对每个文字块都跑一次昂贵的字体分类器；封面页
  被排除在投票样本外（封面通常是标题艺术字，不代表正文）。熔断结果进入**独立**计数器，绝不回流到
  主投票池——否则反复出现的"特殊场景字体"会逐渐挤掉合法的低频正文字体。
- **术语词典保证译名一致。** 人名/术语首次翻译后自动收录，后续精确匹配直接复用；同时把本卷已知
  人名列表（仅名字）随请求发给模型，让它自行识别昵称、缩写等无法用字符串规则可靠匹配的变体。单条
  可改可删，不提供整体清空。
- **成本可见、可控。** 翻译请求按 2–4 页批量发送，且内容顺序固定为"稳定前缀在前、本批文字在后"以
  命中各家的前缀缓存；用量按当前页/当前档案/今日/本周四个粒度随时可查，并可设置每日上限，达到上限
  后自动预读暂停。
- **云端与本地两条路径，各自符合信任边界。** 云端 API Key 只存服务端、绝不下发浏览器，由服务端代理
  调用并在服务端合成图片；本地模型（如 Ollama）则由浏览器直连——服务器本来就访问不到你的回环地址
  ——并在浏览器端用 Canvas 合成，同时把新发现的术语回传服务端共享词典。浏览器拦截本地连接时给出可
  操作的引导（见 `docs/translation-local-backend.md`），而不是让你去关闭浏览器安全特性。
- **翻译结果是正式数据，渲染图只是缓存。** 文字+坐标+样式的 `Detected Text Region` 记录长期持久化并
  纳入备份/导出，渲染好的整页图片则可随时淘汰、随时由这些记录重新合成——一张图片比重建它所需的数据
  大好几个数量级，丢了渲染图只需重跑一次合成，丢了记录却要重跑 OCR 并重新付费翻译。

### 刻意不作为"改进"声明的部分（有意保持对等）

- SHA-1 被保留，而不是升级为更新的哈希算法——见上面的归档 ID 部分；这里的改进在于加入了大小感知的
  *输入*，而不是哈希算法本身。
- RAR/7z 归档仍然通过调用外部 `unrar`/`7z` 处理，与旧版自身的实用做法一致，而不是从零重新实现。
- REST API 契约直接派生自旧版自身的 OpenAPI 规范，只做增量添加——端点形状/路径本身不受影响，明确的目标
  是*不重新设计一套 API*。**唯一的例外是认证机制本身**：旧版 `Bearer base64(apikey)` / `?key=` 方案已
  在发布前被有意替换为上面提到的 JWT + API Token 系统，这是本项目对"保持 API 契约兼容"这条原则唯一的
  刻意背离（详见 `.specify/memory/constitution.md` 对应章程原则末尾的旁注）。
- 搜索引擎是旧版基于 Redis 的模型（有序集合、标签过滤）的直接移植，而不是一项新的搜索技术——这里的实际
  目标就是对等，并已在本项目的目标规模下评估并确认足够。

## 构建与运行

工具链版本固定在 `.mise.toml` 中（`mise install` 会精确复现它们：Rust、Node、Deno、pnpm，以及用于加速
构建的 `sccache`/`mold`）。

```sh
# 后端
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# 前端（开发服务器，将 /api 代理到上面的后端）
cd apps/frontend && pnpm install && pnpm run dev
```

或者通过 Docker（将构建好的前端打包进同一个镜像）：

```sh
docker build -t lanrurugi .
docker run -p 3000:3000 -v /path/to/library:/library lanrurugi
```

一个全新的实例（或者一个从从未修改过密码的旧版安装迁移过来的实例）启动时，仍然使用的是旧版 LANraragi
自身的默认管理员密码 `kamimamita`（`crates/lanrurugi-api/src/auth.rs::DEFAULT_PASSWORD_HASH` 的
bcrypt 哈希对应的明文，与旧版 `Model/Config.pm::get_password` 的默认值一致）。**首次登录后请立即修改
密码**，通过设置页面——不要让一个使用默认密码的实例可以从本地网络外部访问。

### CLI 子命令

- `lanrurugi serve`——在同一进程中运行 HTTP API、静态前端、文件监视器和插件池。
- `lanrurugi rebuild-index`——用大小感知算法重新计算每个归档文件的 ID，并发现任何曾被历史误判合并隐藏的
  文件（用户故事 6）。
- `lanrurugi bench`——生成一个合成库，并针对一个已运行的旧版实例执行并发/吞吐量对比测试（用户故事 8；
  参见 `specs/001-lanrurugi-full-rewrite/quickstart.md` 第 8 节）。

## 测试

```sh
mise run fmt
mise run clippy
mise run test -- -p lanrurugi-translate --lib   # 实际会跑完 workspace；见 .mise.toml 的 --no-fail-fast 说明
```

`scripts/cargo-container-run.sh` 会在 `127.0.0.1:6379` 没有 Redis 时自动起一个临时实例；**但仓库当前的
`.env.test.local` 会把 `LANRURUGI_TEST_REDIS_URL` 覆盖为 `redis://host.docker.internal:16380`**。如果
`16380` 上没有 Redis，`lanrurugi-storage` 的 `bootstrap::...` 测试会失败，表现为该测试二进制
`81 passed; 1 failed`——这是测试环境缺少 16380 Redis，而不是代码失败。启动一个监听 16380 的 Redis 后
即可得到 82/82。

若走裸 `cargo test`，不要设置 `LANRURUGI_TEST_REDIS_URL`，依赖 Redis 的测试会被优雅跳过；设置时请确保
它指向真实可连的 Redis，而不是照抄 README 旧示例里的端口。

### 前端测试（`specs/003-ui-test-automation/`）

```sh
mise run test-frontend-unit   # Vitest + React Testing Library——快速，无需后端
mise run test-frontend-e2e    # Playwright——真实后端 + Redis，Chromium + Firefox
```

`test-frontend-e2e` 会先构建后端，然后为每个测试 worker 启动其自己独立隔离的 Redis 实例、后端进程和
前端预览服务器（见 `apps/frontend/tests/e2e/fixtures.ts`），每次都从一个干净的状态开始。设置
`KEEP=1 mise run test-frontend-e2e` 可以在某一次运行中跳过清理步骤，以便之后检查其环境（Redis/库状态）
——这不会在那一次运行之外持续存在。完整验证指南见 `specs/003-ui-test-automation/quickstart.md`。

## 文档

- [`specs/001-lanrurugi-full-rewrite/`](./specs/001-lanrurugi-full-rewrite/) —— Phase 1 的
  spec、plan、研究决策、数据模型、API 契约，以及 `quickstart.md`（覆盖全部八个用户故事的端到端验证步骤）。
- [`specs/002-job-console/`](./specs/002-job-console/) —— Phase 1 附加功能（增量添加，已实现）：
  呈现现有进程内任务注册表的后台任务管理控制台。
- [`specs/003-ui-test-automation/`](./specs/003-ui-test-automation/) —— Phase 1 附加功能（增量添加，
  已实现）：Vitest + Playwright 自动化前端测试覆盖——见上方的 `## 测试` 一节。
- [`specs/005-download-plugin-progress/`](./specs/005-download-plugin-progress/) —— Phase 1
  附加功能（增量添加，已实现）：为下载插件流水线提供真实的字节级下载进度、按域名的并发限制和限速。
- [`specs/007-guest-restricted-access/`](./specs/007-guest-restricted-access/) —— Phase 1 附加
  功能（增量添加，代码已实现，真机 11 项场景验证待跑）：受限访客访问模式，取代旧版"全开/全锁"式密码开关。
- [`specs/004-ocr-manga-translation/`](./specs/004-ocr-manga-translation/) —— Phase 2（依赖于
  Phase 1，但不会阻塞它；代码已实现，`quickstart.md` 的五个用户故事真机验证待跑，需先准备真实 OCR
  模型文件）：通过 OCR 检测/识别、用户可选的翻译后端（云端或本地托管）以及卷级别的字体匹配实现的
  可选页面内漫画翻译——见上方"页面内漫画翻译（Phase 2）"一节。
- [`docs/translation-local-backend.md`](./docs/translation-local-backend.md) —— 用本地模型
  （如 Ollama）跑页面内翻译的零额外安装配置指南，含浏览器私有网络访问被拦截时的排查步骤。
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) —— 项目治理、架构原则和
  技术栈决策。

## 许可证

MIT——见 [LICENSE](./LICENSE)。
