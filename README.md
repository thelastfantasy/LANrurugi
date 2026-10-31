简体中文 | [English](./README.en.md) | [日本語](./README.ja.md)

# LANrurugi

[![CI](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml)
[![CodeQL](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

自托管的漫画 / 同人志库管理器。Rust 后端 + React 前端，从零重写 [LANraragi](https://github.com/Difegue/LANraragi)。

## 与 LANraragi 的关系

LANrurugi 沿用 LANraragi 的库目录、标签语法和 HTTP API，所以现有的第三方客户端、阅读器和迁移流程都不用改 ——
换过来就是把同一个漫画目录挂上、导入它的备份 JSON。重写的动机是性能与可靠性：后端换成 Rust + Tokio，
缩略图 / OCR / 图像对比这类 CPU 密集路径真正跑满多核，下载传输从插件沙箱搬进宿主进程，才有真实的字节级
进度与按域名限速。能力上采取"对等优先"：LANraragi 已有的行为尽量保持一致，订阅追更、分享导入、页面内翻译、
原生安装包是加在对等之上的东西。逐项对比（包括哪些差异是刻意保留的）见
[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md)。

## 能做什么

- **库与阅读**：分类、标签、阅读进度、全文搜索与标签过滤；书架 / 网格 / 列表视图，界面支持 12 种语言。
- **插件**：元数据、下载、登录三类插件跑在 Deno 子进程里，按命名空间隔离权限；也能让 AI 向导按站点生成新插件。
- **下载**：字节级进度、按域名并发上限与限速；订阅自动追更（发现新作 → 规则判定 → 入队），候选先预览再决定。
- **分享**：把档案或单行本分享给别人，对方在浏览器里读、下载，或用自己的 LANrurugi 直接导入 —— 直连、打洞、
  中继兜底，不依赖第三方服务。
- **页面内翻译**：OCR → 擦除原文 → 排版回填，云端 API 或本地模型均可，字体按卷缓存。
- **电子书**：PDF、EPUB、MOBI/AZW3、DjVu、FB2、TXT 与压缩包一起入库、一样阅读。
- **原生安装包**：Linux（x86_64 / aarch64）、macOS（Apple Silicon）、FreeBSD，解压即用，不需要容器。

## 快速开始

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/manga:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

打开 `http://<服务器地址>:3000`。镜像自带 Redis，不需要额外的数据库容器；更完整的 compose（含 NAS 面板、
反向代理、从 LANraragi 迁移的步骤）见[文档站「安装」](https://docs.lrrg.dev/guide/install/)。

镜像有两个通道：`:latest` 跟随发布版，`:edge` 跟随 `dev` 分支的最新代码（可能坏，别指向正式书库）。

> **默认管理员密码是 `kamimamita`**（与 LANraragi 一致）。首次登录后立刻在设置页改掉，不要把默认密码的实例
> 暴露到局域网外。

## 文档

- 用户指南：<https://docs.lrrg.dev/>
- API 参考：<https://api.lrrg.dev/> —— 由服务端路由自动生成的 OpenAPI，不是手写文档
- 插件 SDK：<https://sdk.lrrg.dev/>
- 给 AI 工具读的入口：<https://lrrg.dev/llms.txt>
- 与 LANraragi 的逐项对比：[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md)

## 参与贡献

见 [CONTRIBUTING.md](./CONTRIBUTING.md)：工具链固定方式、构建与测试命令、提交前必须通过的检查。设计文档在
`specs/` 下，每个功能一份 `spec.md` / `plan.md` / `tasks.md`。

## 变更记录

每次打 `v*` tag，CI 会从提交历史生成中英双语的发布说明，写在 [Releases](https://github.com/thelastfantasy/LANrurugi/releases)
里 —— 应用内的升级提示读的也是这份内容。

## 许可证

MIT —— 见 [LICENSE](./LICENSE)。
