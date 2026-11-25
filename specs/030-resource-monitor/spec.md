# Feature Specification: 应用内资源监视：设置页的资源面板与反馈包里的资源快照

**Feature Branch**: `feat/030-resource-monitor`

**Created**: 2026-11-25

**Status**: Draft

**Input**: 用户连续四条诉求：①「现在为了分析资源占用，我想做一个app内部的资源管理器，并在设置画面显示，对此你有什么建议？我想像firefox或chrome那样提供尽可能详细且精确的情报，并且该资源管理器本身占用资源不高（不在设置页打开的时候就是0占用），有必要使用speckit吗？」②「还有能和sentry进行连携的话是最好的」③「在这里最好也能连着资源占用也一起打包」（附「诊断与反馈 → 反馈包」页签截图：服务端日志 / 前端事件 / 环境与诊断 / 配置(已脱敏) / 超过上限时自动截断）④「检测指标我想到的有cpu，内存，io，gpu，swap。你还有其他补充吗？」。本轮先到 `analyze`，提交一轮待确认后进入实现。

## Context

### 为什么需要它（三次真实事故，全部靠手工采样才定位）

| 事故 | 当时的排查方式 | 缺的是什么 |
|---|---|---|
| 2026-10-11 `mise run test-crate` 连续 exit 137：`memory.high=3G` 时构建 cgroup 长期贴顶、回收停顿上传导致 `systemd-oomd` 杀容器 | 手工 `podman exec` 读 `memory.stat`、数 throttle 事件、读 `systemd-oomd` 日志 | 一个能看到 `memory.high`/`max`/`oom_kill` 计数与 PSI 的页面 |
| 2026-11-25 批量下载时"入库慢"：一次入库对每个插件各起一个声明进程（34 × 并发） | 手工 `for pid in /proc/*` 数 deno 进程、`podman exec` 读 `memory.stat` | 逐插件进程的 pid / RSS / 调用次数 |
| 2026-11-25 用户报"下载时间算进了等待时间" / "N 个传输中不对" | 直接 `curl` 队列接口 + 手工比对 `phases` | 队列与阶段耗时在同一个视图里，且能随反馈包一起发出来 |

### 实测基线（2026-11-24/25，`lrr-dev` 容器，宿主 12 核）

| 观测项 | 数值 | 来源 |
|---|---|---|
| 常驻 Deno worker 单个 RSS | **32–42 MB**（空 `deno eval` 34 MB） | `/proc/<pid>/statm` 逐进程 |
| 5 个并发下载期间的 deno 进程数 | 3 → **4**（只随"用到的不同插件数"增长） | `/proc` 扫描 |
| 同一窗口容器 `memory.current` | 2675 → **4293 MB** | cgroup `memory.current` |
| 同一窗口服务进程 RSS | 1653 → 1808 MB | `/proc/<pid>/status` |
| 插件声明读取：冷 / 缓存命中 | **14.30 ms / 1.848 µs**（≈7 740×） | criterion `plugin_discovery` |
| `GET /api/plugins/metadata`（21 个插件）冷 / 热 | **1.011 s / 0.039 s** | 缓存过期前后各一次 |
| 僵尸 deno（`Z` 状态、已 11.7 h） | 3 个 | 旧二进制上的 `/proc` 扫描 |
| 当前 `diagnostics.json` 在反馈包里的内容 | **0 字节**（`bundle.rs:402` 写入 `Vec::new()`，内容只用于预览算大小） | 代码 + 解包验证 |

### 与宪法的一致性

- **Principle II（LANrarugi 兼容）**：纯新增接口与页面，不改任何既有路由的响应形状；反馈包只是多一个可选文件。
- **Principle III（数据写入）**：资源监视不写任何库/存档记录；历史只存内存，不落 Redis。
- **Principle VI（性能）**：这是本特性的核心约束——没有观察者时不得有采样任务存在。
- 隐私：所有进入反馈包与追踪系统的字段过既有 `diagnostics::scrub`。

## User Scenarios & Testing *(mandatory)*

### User Story 1 - 报障时资源快照跟着反馈包一起发出去 (Priority: P1)

管理员在「诊断与反馈 → 反馈包」里勾上「资源占用」，导出的 ZIP 里除了日志与事件，还有一份 `resources.json`：服务进程内存/线程/fd、cgroup 的 anon/file/peak 与 `oom_kill` 计数、PSI、Tokio 任务数、逐个插件进程的 pid/RSS、队列深度与阶段耗时、Redis 摘要。维护者不用再问"当时机器什么状态"。

**Why this priority**: 这是用户明确要求的落点（③），也是三次事故里最贵的一步——让报障者手工跑 `podman exec` 不现实。它不依赖面板存在，可以单独交付。

**Independent Test**: 只勾「资源占用」导出一次捆绑包，解包后 `resources.json` 存在且包含六个顶层分节；同时 `diagnostics.json` 不再是空文件。

**Acceptance Scenarios**:

1. **Given** 管理员打开反馈包页签，**When** 勾上「资源占用」并下载，**Then** ZIP 内含 `resources.json`，`manifest.json` 的 `files` 列出其字节数，`README.txt` 说明它是什么。
2. **Given** 未勾「资源占用」，**When** 下载，**Then** ZIP 内没有该文件，其余内容不变。
3. **Given** 容器里读不到某个指标（无 PSI 文件 / 无 GPU），**When** 生成快照，**Then** 该字段是 `null` 并带 `unavailable` 原因，其余字段照常。
4. **Given** 快照里出现下载 URL 或库路径，**When** 解包查看，**Then** 与日志同一套脱敏生效（无 access key、无绝对路径中的用户名）。

---

### User Story 2 - 设置页里有一张"谁在吃资源"的表 (Priority: P1)

管理员在设置页打开「资源」面板，看到一张表：服务进程、cgroup、Tokio、每个插件进程、下载队列、Redis 各占一行（或一组），每行有当前值与含义（"含页缓存"这类标注写在同一格）。关掉面板后，服务端不再采样。

**Why this priority**: 用户要求的"Chrome 任务管理器"式体验（①）。没有它，快照只能事后看，定位"现在为什么慢"仍然靠猜。

**Independent Test**: 打开面板 → 表在 1 s 内出现并每秒刷新；关闭面板 → 服务端采样计数停止增长、没有新增定时任务；直接在浏览器里请求流接口也能得到同样的表。

**Acceptance Scenarios**:

1. **Given** 面板打开，**When** 等待 3 s，**Then** 至少刷新 2 次，且每行的"窗口"列显示 `过去 1 s` 或 `自启动累计`。
2. **Given** 面板打开，**When** 关闭它并等 5 s，**Then** 服务端不再产生新样本（可查询一个计数验证），10 分钟内 CPU 占用为 0。
3. **Given** 一个刚启动的新实例，**When** 打开面板，**Then** CPU 列显示 `—`（首帧没有差值），第二帧起才有百分比。
4. **Given** 两个浏览器标签都开着面板，**When** 关掉其中一个，**Then** 另一个继续刷新（采样任务由引用计数持有）。

---

### User Story 3 - 一段时间的曲线，而不只是一瞬间 (Priority: P2)

面板里能看到最近 30 分钟的 CPU / 内存 / IO / 队列深度曲线，用来回答"刚才那波慢是不是内存压力引起的"。历史只活在采样任务里，关掉即丢。

**Why this priority**: 单帧快照解释不了"间歇性慢"；但没有 US1/US2 的话，曲线也没有数据来源。

**Independent Test**: 打开面板跑一次批量下载，30 分钟内曲线有数据；关闭面板再打开，曲线从空开始（不持久化）。

**Acceptance Scenarios**:

1. **Given** 面板打开 10 分钟，**When** 查看曲线，**Then** 时间轴覆盖这 10 分钟且没有缺口（允许 1 帧抖动）。
2. **Given** 全部观察者断开，**When** 重新打开，**Then** 历史从零开始，而不是接上旧的。

---

### User Story 4 - 出问题时追踪系统里能看到资源上下文 (Priority: P2)

后端与前端捕获到错误时，事件上带一份资源上下文（RSS、anon、pids、活跃任务数、常驻插件进程数、PSI 10 s）。阈值被跨过（`oom_kill` 增长、内存 PSI 持续过高）时，主动记一条 warning 诊断事件，也是一份快照。

**Why this priority**: 用户要求的 Sentry 联动（②）。它让"事后追"变成"事件自带现场"，但依赖 US1 的快照能力。

**Independent Test**: 人为触发一次错误事件，在诊断库里看到该事件带 `resources` 上下文；把内存压到超过阈值，看到一条 warning 事件且 10 分钟内不重复。

**Acceptance Scenarios**:

1. **Given** 最近一次采样有效，**When** 记录一条 error 事件，**Then** 事件的 `contexts.resources` 含 6 个数字且有采样时刻。
2. **Given** `memory.events` 的 `oom_kill` 从 0 变 1，**When** 下一次采样命中，**Then** 记一条 warning 诊断事件；紧接着的 10 分钟内不再重复记。
3. **Given** 用户关掉了诊断上报，**When** 发生错误，**Then** 不上报任何资源上下文（纯本地）。

---

### User Story 5 - 把这张表复制给别人 (Priority: P3)

面板与反馈文本都能一键复制成纯文本/CSV，贴进 issue 或聊天，格式稳定（含版本、release、平台与采样窗口）。

**Why this priority**: 前四条已经能自证；复制是让"截图说不清"的场景少一轮往返。

**Independent Test**: 点复制，剪贴板内容是固定表头的文本，粘到任意编辑器里能对齐。

**Acceptance Scenarios**:

1. **Given** 面板打开且有 2 帧数据，**When** 点「复制为文本」，**Then** 剪贴板含版本/release/平台三行与表体。
2. **Given** 面板打开，**When** 点「导出 CSV」，**Then** 下载的文件每行一个时间点、列与表头一致。

---

### Edge Cases

- **没有 cgroup v2 / 没有 PSI 文件 / 没有 GPU**：字段为 `null` 且带 `unavailable` 原因，页面显示 `—` 与原因，不显示 0（0 会被误读）。
- **非 Linux（015 的原生包：macOS / FreeBSD）**：`/proc` 与 cgroup 都不存在；用可移植接口给出进程级 RSS/CPU，其余标 `unavailable`。
- **权限不足**：读不到其它进程的 `/proc/<pid>` 时，只显示本进程与自己的子进程，并在表头注明"仅本进程"。
- **浏览器标签被隐藏**：`visibilitychange` 时暂停采样，恢复时立即取一帧，不留 30 分钟的空洞。
- **多个观察者**：两个标签共享同一个采样任务，引用计数归零才销毁。
- **时钟回拨 / 单调时间不可用**：所有窗口基于单调时钟，展示用墙钟；两者不一致时以单调为准并在 `at` 上标注。
- **快照过大**：单次快照序列化上限 64 KB；超限时丢弃历史与插件明细，保留汇总，并记一条 truncation。
- **实例重启前后**：`oom_kill` 这类计数器从 0 重新开始，面板要标"自本进程启动"，不能与重启前的数字比较。
- **一个插件进程在采样中被替换（文件更新 / 报错重建）**：该行 pid 可能瞬时消失，按"未找到"处理，不报错。

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: 系统 MUST 只有在至少一个观察者存在时才采集资源样本；没有观察者时 MUST NOT 存在采样定时器或后台任务。
- **FR-002**: 最后一个观察者断开后，采样 MUST 在 2 秒内停止，且已采集的历史 MUST 被释放（不落 Redis、不落磁盘）。
- **FR-003**: 每一个数值 MUST 携带采样时刻、窗口长度（如"过去 1 s"/"自本进程启动"）与来源（如"cgroup 内存文件"），页面与导出都保留这三项。
- **FR-004**: cgroup 内存 MUST 拆分为 `anon` / `file` / `slab` 并单独给出峰值；`memory.current` 若展示 MUST 标注"含页缓存"。
- **FR-005**: 快照 MUST 覆盖：CPU（用量与 PSI）、内存（三项拆分、峰值、`high`/`max`/`oom`/`oom_kill` 计数）、IO（读写字节与 PSI）、swap（用量与 PSI full）、磁盘空间与 inode（库盘、temp 盘分开）、fd/线程/进程数、运行时任务与队列深度、逐个插件进程（namespace、pid、RSS、CPU、空闲时长、调用与失败次数）、下载队列深度与阶段耗时（p50/p95）、Redis 摘要（内存、键数、持久化状态）、网络收发速率、缓存命中率、库规模（档案数与总字节，用于归一化）。
- **FR-006**: GPU（显存已用/总量、利用率）与温度 MUST 作为可选项，由 `LANRURUGI_RESOURCE_GPU=1` 显式开启（默认关，因为查询要 spawn `nvidia-smi`）：开启后一次查询的结果 MUST 缓存 5 秒；查不到时 MUST 标 `unavailable` 并说明原因，且 MUST NOT 影响其它字段或让页面报错。
- **FR-007**: 资源面板与资源接口 MUST 仅对管理员可用；访客模式下的任何请求 MUST 被拒绝且不泄露指标。
- **FR-008**: 面板 MUST 支持「复制为文本」与「导出 CSV」，文本含版本、release、平台与采样窗口。
- **FR-009**: 反馈包 MUST 新增「资源占用」类目（默认勾选），生成 `resources.json`；同时 MUST 修复当前 `diagnostics.json` 被写成空文件的问题。
- **FR-010**: 进入反馈包、诊断事件与追踪系统的资源字段 MUST 经过与日志同一套脱敏（无 access key、无 cookie/token、无含用户名的绝对路径）。
- **FR-011**: 记录错误事件时，若最近一次采样仍有效（≤ 30 s），事件 MUST 附带资源上下文（RSS、anon、pids、自统计的任务数、常驻插件进程数、内存 PSI）。
- **FR-012**: 阈值必须被自动监测：`oom_kill`/`oom` 增长、内存 PSI `full` 的 10 秒均值超过 10%、fd 使用超过上限的 80% —— 命中时记一条 warning 诊断事件并附快照，同一阈值 10 分钟内最多一条。
- **FR-013**: 历史 MUST 只存在采样任务的内存里，长度 ≤ 30 分钟；不得写入 Redis、数据库或磁盘。
- **FR-014**: 面板首帧 MUST 在 1 秒内出现；单次采样 MUST 在 50 ms 内完成；采样 MUST NOT 启动任何子进程（除可选的 GPU 查询，且该查询可关闭）。
- **FR-015**: 面板 MUST 在浏览器标签隐藏时暂停采样，并在恢复可见时立即补一帧。
- **FR-016**: 采样 MUST NOT 持有插件运行时的全局锁超过一次读取所需的时间（先取出 pid 列表再逐个读取进程信息）。

### Key Entities

- **ResourceSnapshot**：某一时刻的完整视图。字段：采样时刻、窗口、各分节（服务进程 / cgroup / 运行时 / 插件进程 / 队列 / Redis / 网络 / 缓存 / 库规模）、每个数值的来源与可得性。
- **Sample**：快照的轻量子集（时间 + 少量汇总数字），用于曲线；环形缓冲持有最近 30 分钟。
- **PluginProcess**：一个插件进程的观测：namespace、pid、RSS、CPU、启动时刻、最后调用时刻、调用次数与失败次数、是否常驻。
- **ThresholdAlert**：一次阈值跨越（种类、触发值、时刻、当时的快照摘要），10 分钟内同类只记一次。
- **Observer**：一个正在看数据的客户端（面板会话或一次导出请求）；决定采样任务是否存在。

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 管理员能在两次点击内得到一份含资源快照的反馈包，且解包即能读到六个分节。
- **SC-002**: 面板关闭时该功能的 CPU 占用为 0——连续 10 分钟内采样计数不增长、无新增任务。
- **SC-003**: 面板首帧在 1 秒内出现；打开 30 分钟内曲线无缺口。
- **SC-004**: 单次采样在 50 ms 内完成（20 次连续采样的 p95），且面板打开时的下载平均速率不低于关闭时的 90%（同一批链接各跑一次，quickstart 场景 11 记录两组数字）。
- **SC-005**: 面对"下载变慢"，维护者能在一次快照内判断属于 CPU 压力、IO 压力、网络还是插件进程异常四类之一（清单里每个数字都能对应到一类）。
- **SC-006**: `resources.json` 不超过 64 KB，且在同一实例上重复生成时字段集合稳定（便于比较）。
- **SC-007**: 快照、导出与图表里不出现 access key、cookie、token 与含用户名的绝对路径（用真实样本核验）。

## Assumptions

- 部署形态以单机 / 单容器为主，cgroup v2 + PSI 可用；不可用时降级而不是失败。
- 使用者是管理员本人（本产品自带访客模式，但资源面板只对管理员开放）。
- 不引入 Prometheus / OpenTelemetry 等外部监控栈，也不做常驻 exporter；数据只服务"人工排查 + 报障"两个场景。
- GPU 与温度按"尽力而为"处理：NVIDIA 需要调用外部工具（约 20–40 ms），AMD 可读 sysfs；两者都可能不可用。第二版再做曲线，第一版先给单帧。
- 反馈包上限（默认 20 MB）不变，`resources.json` 计入其中。
- 追踪系统是 Sentry 兼容实现（本仓库自带 rustrak 部署），上报走既有诊断通道的开关。
