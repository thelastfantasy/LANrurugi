---
title: Configuration (environment variables)
description: LANrurugi's command-line arguments, the environment variables behind them, and their defaults.
---

LANrurugi has two kinds of configuration, and keeping them apart saves confusion:

- **Deploy-time** — listen address, directories, a handful of deployment switches. These are `serve`
  command-line flags, each with an environment variable of the same name.
- **Runtime** — library behaviour, sharing, OCR, the AI key and so on. These live in the **settings
  page**, are stored in Redis, take effect immediately, and are not environment variables.

This page covers only the first kind. Precedence is **command line > environment > default**.

The native packages write these into `lanrurugi.env` (the service unit reads it — see
[Native install](../install-native/)); Docker passes them with `-e` or Compose's `environment`. Note
that the image's command already carries `--bind [::]:3000`, and a command-line flag beats an
environment variable, so **`LANRURUGI_BIND` does not work in Docker** — see [Install](../install/)
for how to change the port.

## Server and network

| Variable | Default | Notes |
| --- | --- | --- |
| `LANRURUGI_BIND`<br>`--bind` | `[::]:3000` | Listen address, one whole `address:port`. `[::]` is a dual-stack socket (IPv4 and IPv6); a host without IPv6 falls back to `0.0.0.0` |
| `LANRURUGI_REDIS_URL`<br>`--redis-url` | `redis://127.0.0.1:6379` | `host:port` only, no database index — the five logical databases are chosen by the server itself |
| `LANRURUGI_TELEMETRY_URL`<br>`--telemetry-url` | unset | Sink for the anonymous startup heartbeat (once a day: random install id, version, architecture); unset means nothing is ever sent |
| `LANRURUGI_NO_TELEMETRY`<br>`--no-telemetry` | `false` | Refuses the heartbeat even when the variable above is set |

## Directories

| Variable | Default | Notes |
| --- | --- | --- |
| `LANRURUGI_LIBRARY_PATH`<br>`--library-path` | `./library` | The library; may point straight at LANraragi's `content` |
| `LANRURUGI_THUMB_DIR`<br>`--thumb-dir` | `./thumb` | Thumbnail cache |
| `LANRURUGI_TEMP_DIR`<br>`--temp-dir` | `./temp` | Upload staging, transcoding and render cache; bounded by `tempmaxsize` |
| `LANRURUGI_LOG_DIRECTORY`<br>`--log-dir` | `./log` | The categorised logs behind Settings → Logs |
| `LANRURUGI_PLUGINS_DIR`<br>`--plugins-dir` | `./plugins` | Plugin `.ts` files, namespaced by subdirectory |
| `LANRURUGI_STATIC_DIR`<br>`--static-dir` | unset | Built frontend. Unset serves the API only (the frontend runs from Vite's dev server during development) |
| `LANRURUGI_DOCS_DIR`<br>`--docs-dir` | unset | The documentation site, including the plugin SDK reference, served under `/docs` |

The native packages and the Docker image fill the last two themselves; set them only for your own
packaging or to relocate the files.

### Library scanning

| Variable | Default | Meaning |
| --- | --- | --- |
| `LANRURUGI_SCAN_CONCURRENCY` | half the cores (minimum 2) | How many archives the first full scan processes at once |

Scanning is CPU-heavy (decompress, hash, thumbnail). Too much concurrency saturates the machine and
makes the web UI unusable — the old default was `cores × 4` (48 parallel tasks on 12 cores), which
made a single page load take tens of seconds.

Lower it when the library is large and you want to keep using the instance while it scans:

```yaml
environment:
  - LANRURUGI_SCAN_CONCURRENCY=2
```

The scan also **yields when the instance is busy**. It stops taking new work when either:

- a request arrived within the last 3 seconds, or
- the kernel reports I/O or CPU stalls — read from Linux's PSI (`/proc/pressure/{io,cpu}`), so a ZFS
  scrub, a backup or a neighbour container competing for the disks makes it back off too. Systems
  without PSI only use the request signal.

It never stops completely: **2 archives per minute get through regardless** (a fixed floor rate,
not a timeout). This can be turned off in Settings → "Background workers" → "Idle-time scanning".

## Deployment switches

| Variable | Default | Notes |
| --- | --- | --- |
| `LANRURUGI_FORCE_SECURE_COOKIES`<br>`--force-secure-cookies` | `false` | Adds `Secure` to both auth cookies. Turn it on only once HTTPS is in place, or a browser drops the cookies and login stops working |
| `LANRURUGI_DISABLE_UPDATE_CHECK`<br>`--disable-update-check` | `false` | Suppresses the frontend's new-version check |
| `LANRURUGI_DISABLE_FIRST_RUN_WIZARD`<br>`--disable-first-run-wizard` | `false` | The wizard never opens on its own and the settings page stops offering it. Accepts `1`/`0`, `yes`/`no`, `on`/`off`, `true`/`false` |
| `LANRURUGI_NO_WATCH`<br>`--no-watch` | `false` | Do not watch the library at startup (it can still be started later via `/shinobu/restart`) |
| `LANRURUGI_DENO_BIN`<br>`--deno-bin` | `deno` | Deno executable for plugin subprocesses; resolved from `PATH` by default |

## Optional components

The native packages already point at the files in their own directory, and so does the Docker image;
set these only to use a different copy.

| Variable | Notes |
| --- | --- |
| `LANRURUGI_PDFIUM_LIB` | PDFium shared library (a file, or a directory containing it) |
| `ORT_DYLIB_PATH` | ONNX Runtime shared library, used by OCR, erasing, translation and recommendations |
| `LANRURUGI_MANGA_OCR_MODEL_DIR` | Text-recognition model directory |
| `LANRURUGI_INPAINT_MODEL_DIR` | Erasing (inpainting) model directory |
| `LANRURUGI_BUBBLE_SEG_MODEL_DIR` | Speech-bubble segmentation model; without it translation erases text with rectangles |
| `LANRURUGI_GEOIP_DB_PATH` | GeoLite2-City database, default `/var/lib/GeoIP/GeoLite2-City.mmdb`. A missing file only means activity entries carry no geo data |

## Resources and external services

| Variable | Default | Notes |
| --- | --- | --- |
| `LANRURUGI_BOOK_MAX_WORKERS` | 30% of cores (at least 1) | Number of e-book render subprocesses. Each is a whole process holding PDFium/DjVu state, so a memory-constrained host may want fewer |
| `LANRURUGI_GITHUB_TOKEN` | unset | Only used to raise the GitHub API rate limit during the version check (60 requests/hour anonymously) |
| `DEEPSEEK_API_KEY` | unset | Fallback when no key is set in the settings page, handy for injecting one from Compose |

## Everything else

`LANRURUGI_TRACKER_DSN`, `LANRURUGI_TRACES_SAMPLE_RATE` and `LANRURUGI_SOURCEMAPS_DIR` cover error
reporting and stack-trace symbolication; they matter only when collecting diagnostics for a bug
report against the project.

The remaining `LANRURUGI_*` variables (`LANRURUGI_TELEMETRY_*`, `LANRURUGI_SHARE_NODE_DATA`,
`LANRURUGI_PLUGIN_CONVERTER_BIND` and friends) serve the project's own infrastructure components and
never need setting on a normal deployment; `LANRURUGI_TEST_*` and `TEST_REAL_*` only matter when
running the test suite.
