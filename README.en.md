[简体中文](./README.md) | English | [日本語](./README.ja.md)

# LANrurugi

[![CI](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml)
[![CodeQL](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

A self-hosted manga / doujinshi library manager. Rust backend, React frontend, a from-scratch rewrite of
[LANraragi](https://github.com/Difegue/LANraragi).

## Relationship to LANraragi

LANrurugi keeps LANraragi's library layout, tag syntax and HTTP API, so existing third-party clients,
readers and migration flows keep working — switching over is mounting the same manga directory and
importing a backup JSON. The rewrite is about speed and reliability: the backend is Rust + Tokio, so
CPU-bound paths (thumbnails, OCR, image comparison) actually use every core, and downloads moved out of
the plugin sandbox into the host process, which is what makes real byte-level progress and per-domain
rate limiting possible. Feature-wise it is parity-first: existing LANraragi behaviour is preserved where
it exists, and subscriptions, sharing/import, in-page translation and native packages are added on top.
For the item-by-item comparison — including the differences that are deliberate — see
[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md) (Chinese).

## What it does

- **Library and reading**: categories, tags, reading progress, full-text and tag search; shelf / grid / list
  views, 12 UI languages.
- **Plugins**: metadata, download and login plugins run in Deno subprocesses with per-namespace permissions;
  the AI wizard can generate new ones for a site.
- **Downloads**: byte-level progress, per-domain concurrency and rate limits; subscriptions that discover new
  releases, apply your rules, and queue them after a preview.
- **Sharing**: send an archive or a tankoubon to someone who reads it in the browser, downloads it, or imports
  it into their own instance — direct, hole-punched, or relayed, with no third-party service involved.
- **In-page translation**: OCR → erase the original text → typeset the translation, on a cloud API or a local
  model, with per-volume font caching.
- **E-books**: PDF, EPUB, MOBI/AZW3, DjVu, FB2 and TXT are catalogued and read like any archive.
- **Native packages**: Linux (x86_64 / aarch64), macOS (Apple Silicon) and FreeBSD — unpack and run, no container.

## Quick start

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/manga:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

Open `http://<host>:3000`. The image bundles Redis, so no separate database container is needed; a fuller
compose file (NAS panels, reverse proxy, migrating from LANraragi) is in the
[docs](https://docs.lrrg.dev/en/guide/install/).

Two image channels: `:latest` follows releases, `:edge` follows the head of the `dev` branch (it can break —
don't point a real library at it).

> **The default admin password is `kamimamita`** (same as LANraragi). Change it in Settings right after the
> first login, and don't expose a default-password instance outside your LAN.

## Documentation

- User guide: <https://docs.lrrg.dev/en/>
- API reference: <https://api.lrrg.dev/> — OpenAPI generated from the server's own routes, not hand-written
- Plugin SDK: <https://sdk.lrrg.dev/>
- For AI tools: <https://lrrg.dev/llms.txt>

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) (Chinese) for the pinned toolchain, build/test commands and the checks
a change must pass. Design documents live under `specs/`, one `spec.md` / `plan.md` / `tasks.md` per feature.

## Changelog

Every `v*` tag produces a bilingual release note generated from the commit history — see
[Releases](https://github.com/thelastfantasy/LANrurugi/releases). The in-app upgrade notice reads the same text.

## License

MIT — see [LICENSE](./LICENSE).
