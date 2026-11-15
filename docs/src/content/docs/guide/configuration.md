---
title: 配置项（环境变量）
description: LANrurugi 的命令行参数、对应的环境变量与默认值。
---

LANrurugi 的配置分两类，分开看不容易搞混：

- **部署时配置**——监听地址、各类目录、几个部署级开关。它们是 `serve` 的命令行参数，每个都有一个对应的
  环境变量。
- **运行时配置**——书库行为、分享、OCR、AI 密钥这些。在**设置页面**里改，存进 Redis，改完立即生效，
  既不用重启，也不是环境变量。

这一页只讲第一类。优先级是 **命令行参数 > 环境变量 > 默认值**。

原生安装包把这些变量写在 `lanrurugi.env` 里（服务定义通过 `EnvironmentFile` 读它，见[原生安装](../install-native/)）；
Docker 用 `-e` 或 compose 的 `environment`。注意镜像的启动命令里写死了 `--bind [::]:3000`，命令行参数优先于环境
变量，所以 **`LANRURUGI_BIND` 在 Docker 里不生效**——换端口的方法见[安装](../install/)。

## 服务与网络

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_BIND`<br>`--bind` | `[::]:3000` | 监听地址，写成一整个地址加端口，例如 `0.0.0.0:8080`。`[::]` 是双栈套接字（IPv4 和 IPv6 都在听）；主机没有 IPv6 时退回 `0.0.0.0` |
| `LANRURUGI_REDIS_URL`<br>`--redis-url` | `redis://127.0.0.1:6379` | 只写 `host:port`，不要带库号：五个逻辑库由程序自己选 |
| `LANRURUGI_TELEMETRY_URL`<br>`--telemetry-url` | 未设置 | 匿名启动心跳（每天一次，只含安装 ID、版本、架构）的接收端；不设就完全不发 |
| `LANRURUGI_NO_TELEMETRY`<br>`--no-telemetry` | `false` | 即使上面那个设了也不发 |

## 目录

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_LIBRARY_PATH`<br>`--library-path` | `./library` | 书库目录，可以直接指向旧版 LANraragi 的 `content` |
| `LANRURUGI_THUMB_DIR`<br>`--thumb-dir` | `./thumb` | 缩略图缓存 |
| `LANRURUGI_DATA_DIR` | 未设置 | 设了就把缩略图、Redis 数据库、模型、日志都放到它下面的同名子目录，容器只需挂一个卷（见下方说明） |
| `LANRURUGI_TEMP_DIR`<br>`--temp-dir` | `./temp` | 上传暂存、转码与渲染缓存，受 `tempmaxsize` 限制 |
| `LANRURUGI_LOG_DIRECTORY`<br>`--log-dir` | `./log` | 设置 → 日志页面读的分类日志 |
| `LANRURUGI_PLUGINS_DIR`<br>`--plugins-dir` | `./plugins` | 插件 `.ts` 所在目录，按子目录分命名空间 |
| `LANRURUGI_STATIC_DIR`<br>`--static-dir` | 未设置 | 前端构建产物。不设就只提供 API（开发时前端跑 Vite 的 dev server） |
| `LANRURUGI_DOCS_DIR`<br>`--docs-dir` | 未设置 | 文档站（含插件 SDK 参考），挂在 `/docs` 下 |

原生安装包和 Docker 镜像会把最后两项自己填好，只有自己打包或想换位置时才需要设。

`LANRURUGI_DATA_DIR` 是给容器用的「一个目录装下所有运行数据」开关：设成 `/data` 时，缩略图在
`/data/thumb`、Redis 数据在 `/data/database`（入口脚本会给 `redis-server` 传 `--dir`）、模型在
`/data/models`、日志在 `/data/log`，compose 里只需要

```yaml
volumes:
  - ./data/manga:/home/koyomi/lanrurugi/content
  - ./data/lanrurugi:/data
environment:
  - LANRURUGI_DATA_DIR=/data
```

**不设这个变量时，各路径的默认值与以前完全一致**，老部署不用改；想分层存放（缩略图在 HDD、数据库在
SSD）也继续用逐条挂载的布局。

### 书库里的文件名

LANrurugi 自己写进书库的档案一律按内容命名：`<书库>/<id 前两位>/<id>.<扩展名>`，
旁边放一份 `<id>.json`（记录原始文件名、标题和大小，数据库丢失后用来重建）。因此下载、上传、
分享导入进来的档案叫什么名字都不会影响入库，标题取自原始文件名并去掉 `[中国翻訳]`、`[DL版]`
这类发行标记，想看原标题可以在编辑页改 `title`。

你原来的文件不会被改名：任何不是 `ab/{id}.ext` 的已收录档案（包括升级前就在库里的、以及你之后
通过 SMB、rsync 或直接复制放进书库的），扫描时会连同相对路径一起搬进 `<书库>/_byname/`。
`_byname/` 里的档案按文件名识别，照常浏览、阅读、搜索和下载；补丁文件 `.patch.zip` 跟着档案一起搬。
搬不动时（目标位置已经有一个同名文件）会跳过并在扫描日志里报 `skipped_occupied`，绝不覆盖。

这个约定意味着书库根目录是 LANrurugi 自己管的，请不要把别的工具直接指向根目录下的旧路径
（`_byname/` 下的路径是稳定的，可以指）。

### 日志

日志默认**同时写 stderr 和文件**：按类别分成 `general` / `shinobu` / `plugins` / `redis` /
`http` 五个文件（设置 → 日志页面读的就是这几个），镜像里落在挂载的 `/log` 目录。

文件会自动滚动，不会随运行时间无限增长：

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_LOG_MAX_FILE_MB` | `16` | 单个文件超过这个大小就立刻滚动，不必等到第二天 |
| `LANRURUGI_LOG_MAX_FILES` | `3` | 每个类别保留几个文件（含当前正在写的那个） |

这两项也可以在 **设置 → 后台程序** 里改，改完下一次滚动就按新值来，不需要重启。优先级：
设置页 > 环境变量 > 内置默认（环境变量只是部署时的默认值）。

日志目录必须落在**某个持久化挂载**里。镜像默认写 `/log`（Dockerfile 里设了
`LANRURUGI_LOG_DIRECTORY=/log`），所以 compose 要给它一个卷——单独挂 `./data/log:/log` 最简单；
也可以把 `LANRURUGI_LOG_DIRECTORY` 指到已经挂好的目录下面（比如
`/home/koyomi/lanrurugi/database/log`），这样不用多一个挂载点，代价是日志和 Redis 数据混在同一个
卷里。**只有"写进容器可写层"的日志会在每次升级/重建容器时消失**——容器 stop/start 不会。

`general.log` 是当前文件；跨天（本地时间）或超过大小上限时，它会被改名成
`general-20251105.log`，同一天再滚一次就是 `general-20251105-2.log`，超出的最旧文件直接删除。
所以每个类别最多 `LANRURUGI_LOG_MAX_FILES` 个文件，五个类别合计不超过
`5 × LOG_MAX_FILES × LOG_MAX_FILE_MB`（默认约 240 MB）。

### 书库扫描

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_SCAN_CONCURRENCY` | 核数的一半（至少 2） | 首次全量扫描同时处理多少个档案 |

扫描是 CPU 密集的（解压、算哈希、生成缩略图），并发开太高会把整机占满，连网页都打不开——
旧版本按「核数 × 4」跑，12 核机器上是 48 个任务同时进行，一次页面加载要等几十秒。

书库很大、又想边用边扫时，把它调小：

```yaml
environment:
  - LANRURUGI_SCAN_CONCURRENCY=2
```

扫描用的是**独立的 Redis 连接池**，容量按上面的并发数算，所以即使把并发调得很高，接口请求也不会
排在扫描后面等连接——这是页面转几十秒的另一半原因（连接池被占满），现在两者不再争用。

扫描**忙时会让路**。两种情况会暂停领取新任务：

- 最近 3 秒内有交互请求（页面在翻、接口在被调用）
- 内核报告机器正在等 I/O 或 CPU——读 Linux 的 PSI（`/proc/pressure/{io,cpu}`），所以 ZFS scrub、
  备份任务、别的容器抢磁盘时也会让路；非 Linux 系统没有这个信号，只看请求

但不会彻底停下：**无论多忙，每分钟仍放行 2 个档案**（固定保底速率，而不是等一段时间强行放一个）。
这个行为可以在 设置 →「后台程序」→「闲时扫描」里关掉（关掉后扫描不再避让）。

## 部署开关

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_FORCE_SECURE_COOKIES`<br>`--force-secure-cookies` | `false` | 给两个认证 cookie 加 `Secure`。只在已经走 HTTPS 之后打开，否则浏览器丢弃 cookie、登录不上 |
| `LANRURUGI_DISABLE_UPDATE_CHECK`<br>`--disable-update-check` | `false` | 关掉前端对新版本的检查 |
| `LANRURUGI_DISABLE_FIRST_RUN_WIZARD`<br>`--disable-first-run-wizard` | `false` | 首次运行向导从不自动弹出，设置页也不再提供入口。接受 `1`/`0`、`yes`/`no`、`on`/`off`、`true`/`false` |
| `LANRURUGI_NO_WATCH`<br>`--no-watch` | `false` | 启动时不监视书库目录（之后仍可通过 `/shinobu/restart` 启动） |
| `LANRURUGI_DENO_BIN`<br>`--deno-bin` | `deno` | 插件子进程用的 Deno 可执行文件，默认从 `PATH` 里找 |

## 可选组件

原生安装包会自动指向自己目录下的这些文件，Docker 镜像同样已经配好；只有想换一份时才需要设。

| 变量 | 说明 |
| --- | --- |
| `LANRURUGI_PDFIUM_LIB` | PDFium 共享库（文件，或包含它的目录） |
| `ORT_DYLIB_PATH` | ONNX Runtime 共享库，OCR、擦除、翻译和推荐都用它 |
| `LANRURUGI_MANGA_OCR_MODEL_DIR` | 文字识别模型目录 |
| `LANRURUGI_INPAINT_MODEL_DIR` | 擦除模型目录 |
| `LANRURUGI_BUBBLE_SEG_MODEL_DIR` | 对话框分割模型；没有它时翻译用矩形框擦除原文 |
| `LANRURUGI_GEOIP_DB_PATH` | GeoLite2-City 数据库，默认 `/var/lib/GeoIP/GeoLite2-City.mmdb`。文件不存在就只是活动记录里没有地理位置 |

## 资源与外部服务

| 变量 | 默认值 | 说明 |
| --- | --- | --- |
| `LANRURUGI_BOOK_MAX_WORKERS` | 核心数的 30%（至少 1） | 电子书渲染子进程数。每个都是持有 PDFium/DjVu 状态的完整进程，内存紧张的机器可以调小 |
| `LANRURUGI_GITHUB_TOKEN` | 未设置 | 只用于版本检查时提高 GitHub API 限额（匿名每小时 60 次） |
| `DEEPSEEK_API_KEY` | 未设置 | 设置页面里没填密钥时的回落值，方便用 compose 预先注入 |

## 其它

`LANRURUGI_TRACKER_DSN`、`LANRURUGI_TRACES_SAMPLE_RATE`、`LANRURUGI_SOURCEMAPS_DIR` 用于错误上报与堆栈符号化，
只有给项目报问题、需要采集诊断信息时才用得上。

剩下的 `LANRURUGI_*`（`LANRURUGI_TELEMETRY_*`、`LANRURUGI_SHARE_NODE_DATA`、`LANRURUGI_PLUGIN_CONVERTER_BIND`
等）只服务于项目自身的基础设施组件，普通部署不需要设置；`LANRURUGI_TEST_*` 和 `TEST_REAL_*` 只在跑测试时有意义。
