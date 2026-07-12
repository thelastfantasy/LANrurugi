# LANrurugi

A from-scratch Rust + React rewrite of [LANraragi](https://github.com/Difegue/LANraragi), a
self-hosted manga/doujinshi library manager. Aims for feature parity with, and full data/API
compatibility with, existing LANraragi installations, while fixing a known duplicate-detection
defect and adding genuine multi-core concurrency where the legacy Perl implementation had none.

**Status**: Phase 1 (`specs/001-lanrurugi-full-rewrite/`) is implemented — all eight user stories
(library continuity, non-merging ingestion, third-party API compatibility, plugin metadata
enrichment, backup/export, duplicate repair, UI localization, and a concurrency benchmark against
the legacy system) are done; see `specs/001-lanrurugi-full-rewrite/tasks.md` for the full
breakdown. Phase 2 (`specs/003-ocr-manga-translation/`), on-page manga translation, has a plan but
no implementation yet.

## Stack

- **Backend**: Rust (Tokio async runtime, Axum web framework, Rayon for CPU-bound parallelism),
  one Cargo workspace under `crates/` producing a single binary, `lanrurugi-server`, with
  `serve` / `rebuild-index` / `bench` subcommands.
- **Datastore**: Redis, reused as-is from the legacy deployment (five logical DBs — see
  `crates/lanrurugi-storage/src/redis.rs`).
- **Frontend**: React 19 + TypeScript + Vite + Tailwind + Zustand + TanStack Query, under
  `apps/frontend/`.
- **Plugins**: sandboxed Deno subprocesses, one per plugin namespace (`crates/lanrurugi-plugin/`).
- **Benchmark harness**: `crates/lanrurugi-bench/` — a synthetic-library generator, `criterion`
  microbenchmarks, and a cross-system comparison harness that drives both this binary and an
  actual legacy LANraragi instance side by side.

## Building and running

Toolchain versions are pinned in `.mise.toml` (`mise install` reproduces them exactly: Rust,
Node, Deno, pnpm, plus `sccache`/`mold` for build acceleration).

```sh
# Backend
cargo build --release -p lanrurugi-server
./target/release/lanrurugi-server serve --redis-url redis://127.0.0.1:6379 \
  --library-path /path/to/existing/library

# Frontend (dev server, proxies /api to the backend above)
cd apps/frontend && pnpm install && pnpm run dev
```

Or via Docker (bundles the built frontend into the same image):

```sh
docker build -t lanrurugi .
docker run -p 3000:3000 -v /path/to/library:/library lanrurugi
```

A fresh instance (or one migrated from a legacy install that never changed its password) starts
with legacy LANraragi's own default admin password still in place. **Change it immediately after
first login** via the Settings page — don't leave a default-password instance reachable from
outside your local network.

### CLI subcommands

- `lanrurugi serve` — runs the HTTP API, static frontend, file watcher, and plugin pool in one
  process.
- `lanrurugi rebuild-index` — recomputes every archive's ID with the size-aware algorithm and
  discovers any previously-invisible files a historical false-merge had hidden (User Story 6).
- `lanrurugi bench` — generates a synthetic library and runs the concurrency/throughput
  comparison against an already-running legacy instance (User Story 8;
  see `specs/001-lanrurugi-full-rewrite/quickstart.md` §8).

## Testing

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16379 cargo test --workspace
```

Redis-backed tests are skipped gracefully if `LANRURUGI_TEST_REDIS_URL` is unset; point it at a
scratch Redis instance (e.g. `docker run -d --rm -p 16379:6379 redis:7-alpine`) to run them.

## Documentation

- [`specs/001-lanrurugi-full-rewrite/`](./specs/001-lanrurugi-full-rewrite/) — Phase 1 spec, plan,
  research decisions, data model, API contracts, and `quickstart.md` (end-to-end validation steps
  for all eight user stories).
- [`specs/003-ocr-manga-translation/`](./specs/003-ocr-manga-translation/) — Phase 2 (depends on
  Phase 1, does not block it): optional on-page manga translation via OCR detection/recognition,
  a user-selectable translation backend (cloud or locally-hosted), and volume-level font matching.
- [`.specify/memory/constitution.md`](./.specify/memory/constitution.md) — project governance,
  architectural principles, and technology stack decisions.

## License

MIT — see [LICENSE](./LICENSE).
