简体中文 | [English](./README.en.md) | [日本語](./README.ja.md)

# LANrurugi

[LANraragi](https://github.com/Difegue/LANraragi)（一个自托管的漫画/同人志库管理器）的从零开始的
Rust + React 重写版本。目标是与现有 LANraragi 部署实现功能对等，并保持完整的数据/API 兼容性，同时修复一个
已知的重复检测缺陷，并在旧版 Perl 实现完全没有的地方加入真正的多核并发能力。

**状态**：Phase 1（`specs/001-lanrurugi-full-rewrite/`）已实现——全部八个用户故事（库连续性、非合并式
入库、第三方 API 兼容性、插件元数据增强、备份/导出、重复项修复、UI 本地化，以及一项对比旧系统的并发基准
测试）均已完成；完整拆解见 `specs/001-lanrurugi-full-rewrite/tasks.md`。三个附加于 Phase 1 之上的补充功能
也已全部实现：`specs/002-job-console/`（后台任务管理 UI）、`specs/003-ui-test-automation/`（Vitest +
Playwright 前端测试覆盖），以及 `specs/005-download-plugin-progress/`（真实的字节级下载进度、按域名的并发/
限速）。只有 Phase 2（`specs/004-ocr-manga-translation/`，页面内漫画翻译）仍停留在计划阶段、尚未实现，按照
章程（constitution）原则 VI 被刻意保持独立，使其永远不会阻塞 Phase 1，也不会被 Phase 1 阻塞。

## 技术栈

- **后端**：Rust（Tokio 异步运行时、Axum Web 框架、Rayon 用于 CPU 密集型并行计算），一个位于
  `crates/` 下的 Cargo workspace，产出单一二进制文件 `lanrurugi-server`，带有
  `serve` / `rebuild-index` / `bench` 三个子命令。
- **数据存储**：Redis，原样复用自旧版部署（五个逻辑数据库——见
  `crates/lanrurugi-storage/src/redis.rs`）。
- **前端**：React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query，位于
  `apps/frontend/` 下。
- **插件**：沙箱化的 Deno 子进程，每个插件命名空间一个（`crates/lanrurugi-plugin/`）。
- **基准测试工具**：`crates/lanrurugi-bench/`——一个合成库生成器、`criterion` 微基准测试，以及一个同时驱动
  本二进制文件和一个真实旧版 LANraragi 实例进行并排对比的跨系统对比测试工具。

## 相较 LANraragi 的改进

### 新增功能

- **智能推荐引擎。** 阅读到边界时展示推荐卡片（桌面 10 张/平板 6 张/手机 4 张），本地 ONNX 嵌入模型粗筛 + DeepSeek 大语言模型精排（可配置，不配 Key 也能用纯嵌入排序）；Tankoubon 阅读边界以其最后一卷为锚点计算推荐，并排除同单行本其余卷。
- **真实字节级下载进度与按域名限速。** 下载传输从插件沙箱移至 Rust（流式 reqwest），`downloaded_bytes`/`total_bytes` 实时流入进度条；支持按域名配置并发上限和令牌桶限速，设置项直接展开在插件卡片下方。
- **Tankoubon 连续阅读。** 所有成员档案页面按库内顺序拼接为连续流，翻页无缝跨越成员边界；阅读进度、书签、图章、ToC 等全部正确解析到对应成员档案的本地页码；ToC 优先显示 AI 命名的自定义章节名，未设置时回退到档案标题。
- **档案概览无限滚动。** 页面缩略图按需分批加载而非一次性全部渲染，长篇（尤其多卷 Tankoubon）打开概览不再卡顿；占位符预留真实尺寸避免滚动条跳动，回到顶部按钮重新定位而非级联加载跳过的页面。
- **AI Tankoubon 智能编辑。** 一键分析成员标题中的卷号/话数信息，自动建议单行本名称、章节标题和正确阅读顺序；支持整体命名和逐卷章节名智能建议，多候选翻页预览，即时应用。
- **AI 智能创建 Tankoubon。** 分析尚未加入任何 Tankoubon 的散档案，自动建议可能属于同一系列的分组，也会建议将散档案加入已存在的 Tankoubon；勾选确认后一键创建/添加，纯本地嵌入模型，不依赖 LLM Key。配置 LLM Key 后可选在创建时一并自动应用 AI 重命名与章节排序；不满意的建议可标记"不再提醒"。
- **LLM 标签自动回填。** 用 LLM 从标题中识别并补全缺失的作者/coser 标签，提升分组建议等功能的准确度。
- **更强大的漫画章节（ToC）划分。** 删除/编辑列出档案内全部章节供选择，而非旧版那种只能操作"阅读器当前滚动到哪一章"的限制；封面/封底/目录/彩页/omake/后记/插画等常见章节类型一键预设，或用数字键快捷设置（0 = 目录，1–9 = 对应章节）；预设写入的标题存为内部保留标识符并在重复设置时去重移位（而不是留下过期重复项），前端再映射回本地化显示文本。
- **图章选框标注。** 在单点图章之外支持拖拽矩形选区，可配置颜色/填充/边角/显示方式，8 向手柄缩放、方向键微调、Ctrl+拖拽复制。
- **后台任务管理控制台。** 备份/恢复、缩略图重生成、重复扫描、索引重建等任务的可浏览管理页面，实时状态/进度/结果/过滤。
- **自动化前端测试套件。** Vitest + React Testing Library（单元/逻辑）+ Playwright（Chromium + Firefox 端到端），覆盖所有归档格式和历史已修复 bug 的回归测试。
- **可视化补页。** 上传时遇到内容高度重合的已有档案，自动像素级比对两个压缩包，智能识别相似页面与差异页并以并排预览呈现对齐结果；确认后进入双行缩略图排列界面，通过拖拽将多出的页面插入目标行任意位置，松开即生成精确的页面级补丁文件，无需手动定位或重命名。
- **大量 UI 改进。** 阅读页整页放大预览（lightbox）+ 快速滚动画廊；分类下拉旁快速新建分类入口；Library 网格"标记为已读/未读"右键菜单；搜索语法新增空格作为 AND 分隔符；服务端注入当前主题消除首屏闪烁；集中化路由管理修复多处死链接；`date_added` 按日历日搜索；标签编辑器重建为芯片式输入；搜索过滤器支持浏览器前进/后退；评分标签、Tankoubon 评分清除等若干显示与交互修复。

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

### 刻意不作为"改进"声明的部分（有意保持对等）

- SHA-1 被保留，而不是升级为更新的哈希算法——见上面的归档 ID 部分；这里的改进在于加入了大小感知的
  *输入*，而不是哈希算法本身。
- RAR/7z 归档仍然通过调用外部 `unrar`/`7z` 处理，与旧版自身的实用做法一致，而不是从零重新实现。
- REST API 契约直接派生自旧版自身的 OpenAPI 规范，只做增量添加——明确的目标是*不破坏现有的第三方客户端*，
  而不是重新设计一套 API。
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
自身的默认管理员密码。**首次登录后请立即修改密码**，通过设置页面——不要让一个使用默认密码的实例可以从
本地网络外部访问。

### CLI 子命令

- `lanrurugi serve`——在同一进程中运行 HTTP API、静态前端、文件监视器和插件池。
- `lanrurugi rebuild-index`——用大小感知算法重新计算每个归档文件的 ID，并发现任何曾被历史误判合并隐藏的
  文件（用户故事 6）。
- `lanrurugi bench`——生成一个合成库，并针对一个已运行的旧版实例执行并发/吞吐量对比测试（用户故事 8；
  参见 `specs/001-lanrurugi-full-rewrite/quickstart.md` 第 8 节）。

## 测试

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16379 cargo test --workspace
```

如果未设置 `LANRURUGI_TEST_REDIS_URL`，依赖 Redis 的测试会被优雅地跳过；将其指向一个临时的 Redis 实例
（例如 `docker run -d --rm -p 16379:6379 redis:7-alpine`）以运行这些测试。

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
- [`specs/004-ocr-manga-translation/`](./specs/004-ocr-manga-translation/) —— Phase 2（依赖于
  Phase 1，但不会阻塞它，尚未实现）：通过 OCR 检测/识别、用户可选的翻译后端（云端或本地托管）以及
  卷级别的字体匹配实现的可选页面内漫画翻译。
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) —— 项目治理、架构原则和
  技术栈决策。

## 许可证

MIT——见 [LICENSE](./LICENSE)。
