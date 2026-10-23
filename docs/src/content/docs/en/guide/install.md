---
title: Install (Docker / NAS)
description: Deploy LANrurugi on a server or NAS with Docker or Compose.
---

The LANrurugi server is one resident process holding the static frontend, the HTTP API, the file
watcher and background jobs; plugins, e-book parsing and AI inference run in subprocesses started on
demand. It has its own database and neither reads nor changes LANraragi's, so the two can run side by side.

:::tip[Moving from LANraragi]
1. In LANraragi, go to Settings → Database Backup/Restore → Backup Database and save the JSON. LANraragi
   can keep running.
2. Start LANrurugi with the same manga directory as its library. Thumbnails and the database need mounts
   too, but in new directories, not LANraragi's. The converter on the
   [homepage](https://lrrg.dev/en/#start) generates this from LANraragi's deployment
   command or compose file.
3. Sign in with the default password `kamimamita`; the first-run wizard asks you to change it.
4. Upload the JSON in the wizard's "Import from a LANraragi backup" step (or later under Settings →
   Database Backup/Restore). Tags, categories, tankoubons and stamps come back; reading progress is not
   part of the backup.
:::

Prefer not to use containers? There are [native packages](../install-native/) for Linux, macOS (Apple
Silicon) and FreeBSD.

## Deploy with Compose (recommended)

Save the following as `compose.yaml`, change the host directories on the left as the comments
describe, then start it:

```yaml
services:
  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:latest
    container_name: lanrurugi
    ports:
      - "3000:3000"
    volumes:
      # Library — can point straight at LANraragi's content directory
      - ./data/manga:/home/koyomi/lanrurugi/content
      # Thumbnail cache
      - ./data/thumb:/home/koyomi/lanrurugi/thumb
      # Redis data, LANrurugi only; LANraragi's metadata is imported from its backup JSON
      - ./data/database:/home/koyomi/lanrurugi/database
      # Embedding model for recommendations, downloaded on first use
      - ./data/models:/home/koyomi/lanrurugi/models
    environment:
      # Set to true when served over HTTPS through a reverse proxy
      - LANRURUGI_FORCE_SECURE_COOKIES=false
    restart: unless-stopped
```

```sh
docker compose up -d      # or: podman compose up -d
```

On a NAS (Synology Container Manager, Portainer and the like), create a compose project and paste
the same text.

Then open `http://<server>:3000` in a browser. Redis is bundled in the image; no separate database
container is needed. The four mounted directories:

| Host path | Path in the container | Purpose |
|---|---|---|
| `./data/manga` | `/home/koyomi/lanrurugi/content` | Your library |
| `./data/thumb` | `/home/koyomi/lanrurugi/thumb` | Thumbnail cache |
| `./data/database` | `/home/koyomi/lanrurugi/database` | Redis data (LANrurugi only) |
| `./data/models` | `/home/koyomi/lanrurugi/models` | Embedding model cache for recommendations |

To serve it on a domain over HTTPS, see [Reverse proxy](../reverse-proxy/).

## Run it with plain Docker

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/library:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

## Where the image is published

The same image, for amd64 and arm64, is published to two registries:

| Registry | Image |
| --- | --- |
| GitHub Container Registry (recommended) | `ghcr.io/thelastfantasy/lanrurugi` |
| Docker Hub | `jade913/lanrurugi` |

The examples above use GitHub's registry. Docker Hub limits how often anonymous users can pull; if
your NAS panel only searches Docker Hub, or you simply prefer it, use `jade913/lanrurugi` instead.
`latest` points at the newest release; a specific version number works too.

## Build the image from source

The repository's own `compose.yaml` builds from source for a trial run (`network_mode: host`,
listening on port 3000):

```sh
podman compose up -d --build      # or: docker compose up -d --build
```

## First login

The first visit opens the first-run wizard: change the admin password, add a DeepSeek API key for
the AI features, and import a LANraragi backup. Every step can be skipped, and the wizard can be
rerun from Settings later.

A brand-new instance — or one migrated from an old installation whose password was never changed —
uses LANraragi's own default administrator password, **`kamimamita`**.

:::caution[Change the password first thing]
Change it in the settings page immediately after logging in. Do not leave an instance reachable from
outside your local network while it still uses the default password.
:::

## Reading this documentation offline

The documentation site, the API reference, the Swagger UI and the plugin SDK reference are all baked
into the image and served by the instance itself under `/docs`:

```text
http://<your instance address>:3000/docs/             guides and the per-group API reference (Chinese)
http://<your instance address>:3000/docs/en/          the same, in English
http://<your instance address>:3000/docs/swagger/     Swagger UI, Chinese content (schemas; send requests)
http://<your instance address>:3000/docs/swagger/en/  Swagger UI, English content
http://<your instance address>:3000/docs/sdk/         plugin SDK reference
```

No internet access is required: even Swagger UI's JavaScript bundle and the SDK's search index ship
inside the image — nothing is loaded from a CDN.

## Toolchain (building from source)

Versions are pinned in `.mise.toml`, and `mise install` reproduces them exactly (Rust, Node, Deno,
pnpm, plus `sccache`/`mold`).

```sh
# Backend
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# Frontend dev server (proxies /api to the backend above)
pnpm --filter lanrurugi-frontend run dev
```

### CLI subcommands

- `lanrurugi serve` — runs the HTTP API, static frontend, file watcher and plugin pool in one process.
- `lanrurugi rebuild-index` — recomputes every archive's ID with the size-aware algorithm and surfaces
  files that were historically hidden by a mistaken merge.
- `lanrurugi bench` — generates a synthetic library and runs a concurrency/throughput comparison
  against a running legacy instance.

## Next

- [Quickstart](../quickstart/)
- [Testing the API from these docs](../api-testing/)
