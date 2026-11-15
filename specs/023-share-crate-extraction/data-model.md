# Phase 1 — Data Model: Share Crate Extraction

This feature has no new persistent data. What follows is the shape of the code boundary: which module owns what, and
which data crosses the boundary in which direction.

## Crate / module ownership

```text
lanrurugi-share
├── lib.rs         ShareEngine (start/stop/status/rotate), re-exports of public types
├── host.rs        ShareHost + the five source/sink traits
├── engine.rs      wiring: net + nodes + import/export + settings, task ownership
├── net/           endpoint, transports, server (range serving), relay lifecycle
├── nodes.rs       node record build/publish/refresh, allow-list parsing
├── import.rs      import state machine (claim, manifest, chunks, resume)
├── export.rs      share creation, ticket, session slots, ratios
├── settings.rs    ShareSettings struct, defaults, validation
└── recipient.rs   recipient-side URL/base helpers
```

| Today | After |
|---|---|
| `crates/lanrurugi-api/src/share_net/` (~7,100 lines) | `crates/lanrurugi-share/src/net/` + `import.rs`/`export.rs`/`nodes.rs` |
| `share_settings.rs` (settings struct + validation) | `lanrurugi-share/src/settings.rs` (struct) + `lanrurugi-api/src/share_settings.rs` (HTTP only) |
| `share_recipient.rs` (helpers) | `lanrurugi-share/src/recipient.rs`; the routers stay in the API |
| `shares.rs`, `share_imports.rs` (routers) | unchanged in `lanrurugi-api` |
| 56 `AppState` uses | calls on `ShareHost` |

## The boundary

`ShareHost` is the only thing the engine knows about the application:

| Accessor | Methods | Direction |
|---|---|---|
| `archives()` | `path(id)`, `metadata(id)`, `page_count(id)`, `open(id)` | engine asks, API answers |
| `downloads()` | `enqueue(request)`, `progress(handle)` | both ways |
| `tankoubons()` | `manifest(id)` | engine asks |
| `activity()` | `record(event)` | engine reports |
| `settings()` | `share()`, `general()` | engine reads |

Rules:

- Values crossing the boundary are plain data (`ArchiveId`, `PathBuf`, `u64`, `ShareSettings`), never `axum` types, never
  an API error enum, never a `Json<…>` wrapper.
- The host is passed once when the engine starts and stored as `Arc<dyn ShareHost>`; per-chunk paths never touch it.
- A host method that would block does its blocking work on the API side (spawn_blocking), not in the engine.

## Lifecycle and ownership

```text
main:
  build state (repos, queue, activity, settings)
  let host = Arc::new(ApiShareHost::new(state.clone()));
  let share = ShareEngine::new(host, share_settings);
  share.start().await;        // same point share_net::start() is called today
  ...serve HTTP...
  share.stop().await;         // same point as today
```

- The engine owns its background tasks (endpoint watch, record refresh, relay serve, import drivers), exactly as
  today's `share_net` does.
- The API owns its tasks (download queue, job registry) and exposes them only through the host.
- Restarting the HTTP router never restarts the engine; restarting the engine never rebuilds the state.

## Errors

- Engine errors are the engine's own enum (`ShareError`), mapped to HTTP status codes by the API layer.
- Host errors are `anyhow::Error`-level opaque failures; the engine turns them into `ShareError::Host`.
- No engine error variant may carry an `axum` response.

## Compatibility invariants (checked in review)

1. Redis keys unchanged.
2. Wire formats and `SHARE_ENGINE_VERSION` unchanged.
3. Settings keys and defaults unchanged.
4. Route table unchanged (docgen diff empty).
5. Number and order of background tasks unchanged.
6. `lanrurugi-share` has no path to `lanrurugi-api`.
