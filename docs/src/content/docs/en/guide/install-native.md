---
title: Native install (Linux / macOS / FreeBSD)
description: Run LANrurugi directly on Linux, an Apple Silicon Mac or FreeBSD, without containers.
---

Besides the [Docker image](../install/), every release ships native packages for:

| Package | Systems |
| --- | --- |
| `lanrurugi-<version>-linux-x86_64.tar.gz` | 64-bit Linux with glibc 2.35 or newer (Debian 12, Ubuntu 22.04, Fedora 36, Arch, …) |
| `lanrurugi-<version>-linux-aarch64.tar.gz` | ARM64 Linux, same baseline (e.g. Raspberry Pi 5, ARM servers) |
| `lanrurugi-<version>-macos-arm64.tar.gz` | Apple Silicon Macs, macOS 13 or newer |
| `lanrurugi-<version>-freebsd-x86_64.tar.gz` | FreeBSD 14 |

Download them from the GitHub [Releases](https://github.com/thelastfantasy/LANrurugi/releases) page;
`SHA256SUMS` lists every file's checksum.

Docker remains the least work if you would rather not manage dependencies; on Windows, use Docker
Desktop.

## 1. Install the prerequisites

The database server, plugin runtime, DjVu tools and CJK fonts come from your system's package manager
and are not in the package:

| | Debian / Ubuntu | Fedora | Arch | macOS (Homebrew) | FreeBSD |
| --- | --- | --- | --- | --- | --- |
| Redis 7.4 or newer | `apt install redis-server` ¹ | `dnf install redis` ² | `pacman -S redis` | `brew install redis` | `pkg install redis` |
| Deno (plugins) | [install script](https://deno.com) | [install script](https://deno.com) | `pacman -S deno` | `brew install deno` | `pkg install deno` |
| DjVu books | `apt install djvulibre-bin` | `dnf install djvulibre` | `pacman -S djvulibre` | `brew install djvulibre` | `pkg install djvulibre` |
| CJK fonts (text books) | `apt install fonts-noto-cjk` | `dnf install google-noto-serif-cjk-fonts google-noto-sans-cjk-fonts` | `pacman -S noto-fonts-cjk` | built in | `pkg install noto-serif-sc noto-sans-sc noto-serif-jp noto-sans-jp` |
| Archive codecs | built in | built in | built in | `brew install xz lz4 zstd` | `pkg install lz4 zstd` |
| PDF books | bundled | bundled | bundled | bundled | `pkg install pdfium` |
| OCR, translation, recommendations | bundled | bundled | bundled | bundled | `pkg install onnxruntime` |

¹ Debian 12 ships Redis 7.0, which is fine for a new library; reusing data from a Docker deployment
needs 7.4 or newer (use the redis.io repository).
² Recent Fedora installs Valkey, a Redis-compatible server. It is fine for a new library but cannot
open data files written by Redis 7.4.

## 2. Unpack and start

```sh
tar -xzf lanrurugi-<version>-<platform>.tar.gz
cd lanrurugi-<version>-<platform>
redis-server --daemonize yes      # or enable your system's Redis service
./bin/lanrurugi-server serve \
  --library-path ~/manga \
  --thumb-dir ~/.local/share/lanrurugi/thumb \
  --temp-dir ~/.local/share/lanrurugi/temp \
  --log-dir ~/.local/share/lanrurugi/log
```

Then open `http://<your machine>:3000`. The first-run wizard changes the default password, takes the
API key for the AI features and can import a LANraragi backup.

The web interface, plugins, documentation, bundled libraries and models are found relative to the
program itself, so the package works wherever you unpack it. It connects to
`redis://127.0.0.1:6379` by default; change that with `--redis-url` or `LANRURUGI_REDIS_URL`.

## 3. Translation models (optional)

```sh
./fetch-models.sh                  # text recognition and removal, several hundred MB
./fetch-models.sh --with-bubbles   # also builds the speech-bubble model (needs uv)
```

The library and reader work without them; without the bubble model, translation erases text with
rectangles.

## 4. Run as a service

```sh
sudo ./install-service.sh /var/lib/lanrurugi   # Linux (systemd), FreeBSD (rc.d)
./install-service.sh ~/LANrurugi                # macOS (launchd, current user)
```

The script creates the data directories and `lanrurugi.env` (library location, Redis URL, port),
installs the service definition for your system and prints the command that starts it. Add `--print`
to see the generated definition without installing it.

## Moving from a Docker deployment

Reuse the container's three mounted directories: `content` as the library and `thumb` as the
thumbnail directory. Give the `database` directory to a Redis 7.4+ configured like
`service/redis.conf.example` (its `dir` pointing there, port 16379), and set `LANRURUGI_REDIS_URL` to
`redis://127.0.0.1:16379`.

## Features by platform

| Feature | Linux | macOS | FreeBSD |
| --- | --- | --- | --- |
| Library, reader, search, archives | ✓ | ✓ | ✓ |
| PDF / DjVu / EPUB / MOBI / FB2 / TXT | ✓ | ✓ | ✓ (PDF needs the `pdfium` package) |
| Plugins, downloads, subscriptions | ✓ | ✓ | ✓ |
| Recommendations, OCR, in-page translation | ✓ | ✓ (CPU) | ✓ (CPU; needs the `onnxruntime` package) |
| GPU-accelerated inference | ✓ (as in the Docker image) | — | — |

When an optional component is missing the server still starts; its start-up log prints
`optional components` and `some features are unavailable on this system`, naming what is missing and
which features it affects, and those features report the reason when used.

## Platforms not supported yet

- **Windows**: use Docker Desktop.
- **Intel Macs, other BSDs, musl distributions (e.g. Alpine)**: no packages; musl distributions can
  use the Docker image.
- **HarmonyOS / OpenHarmony**: not supported. Apps there run in a sandbox with no supported way to keep
  a server running in the background, and none of Redis, Deno, PDFium or ONNX Runtime has a HarmonyOS
  build. The realistic route is a separate HarmonyOS reader app that connects to an existing
  LANrurugi server, rather than running the server on the device.
