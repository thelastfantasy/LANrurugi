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
| `LANRURUGI_TEMP_DIR`<br>`--temp-dir` | `./temp` | 上传暂存、转码与渲染缓存，受 `tempmaxsize` 限制 |
| `LANRURUGI_LOG_DIRECTORY`<br>`--log-dir` | `./log` | 设置 → 日志页面读的分类日志 |
| `LANRURUGI_PLUGINS_DIR`<br>`--plugins-dir` | `./plugins` | 插件 `.ts` 所在目录，按子目录分命名空间 |
| `LANRURUGI_STATIC_DIR`<br>`--static-dir` | 未设置 | 前端构建产物。不设就只提供 API（开发时前端跑 Vite 的 dev server） |
| `LANRURUGI_DOCS_DIR`<br>`--docs-dir` | 未设置 | 文档站（含插件 SDK 参考），挂在 `/docs` 下 |

原生安装包和 Docker 镜像会把最后两项自己填好，只有自己打包或想换位置时才需要设。

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
