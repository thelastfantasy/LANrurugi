---
title: Install (Docker / NAS)
description: Deploy LANrurugi on a server or NAS with Docker or Compose.
---

LANrurugi is a single binary: the static frontend, the HTTP API, the file watcher and the plugin pool
all live in one process. It reuses the Redis you already have and does **not** destructively migrate
existing data — the `data/database` directory mounted by `compose.yaml` is the same Redis data
directory the old LANraragi used.

## Start it with Compose

```sh
podman compose up -d      # or: docker compose up -d
```

The three host directories `compose.yaml` uses match the old LANraragi `data/` layout:

| Host path | Path in the container | Purpose |
|---|---|---|
| `./data/manga` | `/home/koyomi/lanrurugi/content` | Your library |
| `./data/thumb` | `/home/koyomi/lanrurugi/thumb` | Thumbnail cache |
| `./data/database` | `/home/koyomi/lanrurugi/database` | Redis data (shared with the old instance) |

The container uses `network_mode: host`, and the server listens on port **3000** (`LANRURUGI_BIND`
defaults to `0.0.0.0:3000`).

## Run it with plain Docker

```sh
docker build -t lanrurugi .
docker run -p 3000:3000 -v /path/to/library:/library lanrurugi
```

## First login

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
