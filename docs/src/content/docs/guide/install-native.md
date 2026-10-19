---
title: 原生安装（Linux / macOS / FreeBSD）
description: 不用容器，直接在 Linux、Apple Silicon Mac 或 FreeBSD 上运行 LANrurugi。
---

除了 [Docker 镜像](../install/)，每个版本还提供以下平台的原生安装包：

| 安装包 | 适用系统 |
| --- | --- |
| `lanrurugi-<版本>-linux-x86_64.tar.gz` | 64 位 Linux（glibc 2.35 及以上：Debian 12、Ubuntu 22.04、Fedora 36、Arch 等） |
| `lanrurugi-<版本>-linux-aarch64.tar.gz` | ARM64 Linux（同上，例如树莓派 5、ARM 服务器） |
| `lanrurugi-<版本>-macos-arm64.tar.gz` | Apple Silicon Mac（macOS 13 及以上） |
| `lanrurugi-<版本>-freebsd-x86_64.tar.gz` | FreeBSD 14 |

安装包在 GitHub 的 [Releases](https://github.com/thelastfantasy/LANrurugi/releases) 页面，`SHA256SUMS` 里有每个文件的校验值。

不想管理依赖的话，Docker 仍然是最省事的方式；Windows 用户请使用 Docker Desktop。

## 1. 安装依赖

数据库、插件运行环境、DjVu 工具和中日韩字体由系统的包管理器安装，安装包里不附带：

| | Debian / Ubuntu | Fedora | Arch | macOS（Homebrew） | FreeBSD |
| --- | --- | --- | --- | --- | --- |
| Redis 7.4 及以上 | `apt install redis-server` ¹ | `dnf install redis` ² | `pacman -S redis` | `brew install redis` | `pkg install redis` |
| Deno（插件） | [官方安装脚本](https://deno.com) | [官方安装脚本](https://deno.com) | `pacman -S deno` | `brew install deno` | `pkg install deno` |
| DjVu 阅读 | `apt install djvulibre-bin` | `dnf install djvulibre` | `pacman -S djvulibre` | `brew install djvulibre` | `pkg install djvulibre` |
| 中日韩字体（文字书） | `apt install fonts-noto-cjk` | `dnf install google-noto-serif-cjk-fonts google-noto-sans-cjk-fonts` | `pacman -S noto-fonts-cjk` | 系统自带，无需安装 | `pkg install noto-serif-sc noto-sans-sc noto-serif-jp noto-sans-jp` |
| 压缩包解码库 | 系统自带 | 系统自带 | 系统自带 | `brew install xz lz4 zstd` | `pkg install lz4 zstd` |
| PDF 阅读 | 安装包自带 | 安装包自带 | 安装包自带 | 安装包自带 | `pkg install pdfium` |
| OCR、翻译、推荐 | 安装包自带 | 安装包自带 | 安装包自带 | 安装包自带 | `pkg install onnxruntime` |

¹ Debian 12 自带的是 Redis 7.0，用于新书库没有问题；要沿用 Docker 部署留下的数据，需要 7.4 及以上（可使用 redis.io 的软件源）。
² 较新的 Fedora 实际安装的是兼容 Redis 的 Valkey，用于新书库没有问题，但它读不了 Redis 7.4 写出的数据文件。

## 2. 解压并启动

```sh
tar -xzf lanrurugi-<版本>-<平台>.tar.gz
cd lanrurugi-<版本>-<平台>
redis-server --daemonize yes      # 或启用系统自带的 Redis 服务
./bin/lanrurugi-server serve \
  --library-path ~/manga \
  --thumb-dir ~/.local/share/lanrurugi/thumb \
  --temp-dir ~/.local/share/lanrurugi/temp \
  --log-dir ~/.local/share/lanrurugi/log
```

然后打开 `http://<你的机器>:3000`，首次运行向导会引导你修改默认密码、填写 AI 功能用的 API 密钥、导入 LANraragi 备份。

前端、插件、文档、自带的原生库和模型都按程序自己的位置查找，解压到哪里都可以，不需要再设置其它路径。
默认连接 `redis://127.0.0.1:6379`，用 `--redis-url` 或环境变量 `LANRURUGI_REDIS_URL` 修改。

## 3. 下载翻译模型（可选）

```sh
./fetch-models.sh                  # 文字识别与擦除模型，几百 MB
./fetch-models.sh --with-bubbles   # 另外生成对话框分割模型（需要 uv）
```

不下载也能正常使用书库和阅读器；没有对话框分割模型时，翻译用矩形框擦除原文。

## 4. 作为系统服务运行

```sh
sudo ./install-service.sh /var/lib/lanrurugi   # Linux（systemd）、FreeBSD（rc.d）
./install-service.sh ~/LANrurugi                # macOS（launchd，当前用户）
```

脚本会创建数据目录和 `lanrurugi.env`（书库位置、Redis 地址、监听端口），并为当前系统安装服务定义，最后提示启动命令。
只想看生成的服务文件而不安装，加 `--print`。

## 从 Docker 部署迁移

把容器挂载的三个目录直接拿过来用：`content` 作为书库，`thumb` 作为缩略图目录。`database` 目录交给一个按
`service/redis.conf.example` 配置的 Redis（7.4 及以上），它的 `dir` 指向这个目录，端口 16379；再把
`LANRURUGI_REDIS_URL` 设为 `redis://127.0.0.1:16379`。

## 各平台功能

| 功能 | Linux | macOS | FreeBSD |
| --- | --- | --- | --- |
| 书库、阅读、搜索、压缩包 | ✓ | ✓ | ✓ |
| PDF / DjVu / EPUB / MOBI / FB2 / TXT | ✓ | ✓ | ✓（PDF 需 `pdfium` 包） |
| 插件、下载、订阅 | ✓ | ✓ | ✓ |
| 推荐、OCR、页面翻译 | ✓ | ✓（CPU） | ✓（CPU，需 `onnxruntime` 包） |
| GPU 加速推理 | ✓（与 Docker 镜像相同） | — | — |

缺少某个可选组件时服务照常启动，启动日志会列出 `optional components` 和
`some features are unavailable on this system`，说明缺了什么、影响哪些功能；用到这些功能时也会显示原因。

## 尚未支持的平台

- **Windows**：请使用 Docker Desktop。
- **Intel Mac、其它 BSD、musl 发行版（如 Alpine）**：没有提供安装包；musl 发行版请使用 Docker 镜像。
- **鸿蒙（HarmonyOS / OpenHarmony）**：暂不支持。鸿蒙系统上的应用运行在沙箱里，不支持长期在后台运行的
  服务端程序，LANrurugi 依赖的 Redis、Deno、PDFium、ONNX Runtime 也都没有鸿蒙版本。可行的方向是另做
  一个连接现有 LANrurugi 服务端的鸿蒙阅读客户端，而不是在设备上运行服务端。
