---
title: 给 AI 工具用的 llms.txt
description: 项目主页和文档站提供的 llms.txt 系列文件各自包含什么，以及让 AI 助手了解 LANrurugi 时该给它哪一份。
---

[llms.txt](https://llmstxt.org/) 是一种约定：在站点根目录放一份 Markdown 文件，告诉 AI 工具这个项目是什么、
更详细的内容去哪里读。LANrurugi 的项目主页和文档站都提供了这类文件。把对应的地址交给 AI 助手，
它就能按文档回答问题、帮你部署、写调用 API 的脚本或者写插件，而不必只凭训练时的印象猜。

## 文件一览

| 地址 | 内容 | 大小（约） |
|---|---|---|
| [`lrrg.dev/llms.txt`](https://lrrg.dev/llms.txt) | 入口：项目简介、HTTP API 与插件 SDK 的说明，以及主要文档的链接 | 3.5 KB |
| [`docs.lrrg.dev/llms.txt`](https://docs.lrrg.dev/llms.txt) | 文档站目录：下面各文件的链接，以及 API 与 SDK 的说明 | 3 KB |
| [`docs.lrrg.dev/llms-full.txt`](https://docs.lrrg.dev/llms-full.txt) | 全部文档页，中英两种语言，合成一个 Markdown 文件 | 800 KB |
| [`docs.lrrg.dev/llms-small.txt`](https://docs.lrrg.dev/llms-small.txt) | 精简版：只含指南和插件开发，去掉 API 参考页与提示框 | 80 KB |
| [`…/_llms-txt/guides-simplified-chinese.txt`](https://docs.lrrg.dev/_llms-txt/guides-simplified-chinese.txt) | 只含中文的指南与插件开发 | 40 KB |
| [`…/_llms-txt/guides-english.txt`](https://docs.lrrg.dev/_llms-txt/guides-english.txt) | 同上，英文 | 40 KB |
| [`…/_llms-txt/api-reference-simplified-chinese.txt`](https://docs.lrrg.dev/_llms-txt/api-reference-simplified-chinese.txt) | 只含中文的 API 参考，每组一节 | 300 KB |
| [`…/_llms-txt/api-reference-english.txt`](https://docs.lrrg.dev/_llms-txt/api-reference-english.txt) | 同上，英文 | 300 KB |

另有两份结构化数据，llms.txt 里也有链接：

- [`docs.lrrg.dev/openapi.json`](https://docs.lrrg.dev/openapi.json)：完整的 HTTP API，OpenAPI 3.1 格式。它由服务器
  自己注册的路由生成，列出的正是服务器实际提供的全部端点。
- [`sdk.lrrg.dev/plugin-sdk.json`](https://sdk.lrrg.dev/plugin-sdk.json)：插件 SDK 参考的 JSON 版，由 `deno doc`
  从插件运行时实际加载的 TypeScript 生成。

这些文件在每次发布文档时与网页一同重新生成，内容和网页同源，不会各自过时。

## 该给 AI 哪一份

| 你想让 AI 做什么 | 给它 |
|---|---|
| 介绍这个项目、回答一般问题 | `lrrg.dev/llms.txt` |
| 帮你部署、排查安装与反向代理问题 | 一种语言的指南（`guides-*.txt`）或 `llms-small.txt` |
| 写调用 API 的脚本或客户端 | `openapi.json`：比 API 参考页的 Markdown 更精确，也更省 token |
| 写插件 | [插件开发入门](../../plugin/getting-started/) 加上 `plugin-sdk.json` |
| 需要全部背景、上下文足够大 | `llms-full.txt` |

如果只是想为某个站点生成元数据、登录或下载插件，实例里自带的 **「AI 创建插件」** 会直接抓取目标页面、
生成代码并试运行，通常比让外部 AI 助手从零写更省事。

## 怎么交给 AI

- **能联网的 AI 工具**（带网页读取的聊天助手、编程代理等）：直接把地址发给它，例如
  「先读 `https://lrrg.dev/llms.txt`，再回答……」。它会按里面的链接继续读需要的部分。
- **不能联网的工具**：先下载需要的文件，再作为附件上传或粘贴进对话。
- **编程代理**（在代码仓库里工作的 AI）：把 `openapi.json` 或 `plugin-sdk.json` 下载进项目目录，比每次
  联网读取更稳定。
