# Phase 0 — Research: Share Crate Extraction

## R1. Move order: leaves, engine, adapter, tests

**Decision**: four commits that each leave the workspace green.

1. Types that both sides use move down (`ArchiveId` and permission/record types to `lanrurugi-core`, share settings to
   `lanrurugi-share` once it exists), with `pub use` in `lanrurugi-api` so no caller changes in that commit.
2. `share_net/` moves to `crates/lanrurugi-share/src/net/`; `AppState` parameters are replaced by a `&dyn ShareHost`
   threaded from the engine handle.
3. `lanrurugi-api/src/share_host.rs` implements the trait; routers keep their current call sites.
4. Tests move, the sharing tests get a fake host, the matrix script and docgen gate are run.

**Rationale**: a single "move everything" commit cannot be reviewed and cannot be bisected when the matrix fails.

## R2. What the host trait exposes

**Decision**: one trait, grouped accessors, not one method per call site:

```rust
pub trait ShareHost: Send + Sync {
    fn archives(&self) -> &dyn ArchiveSource;    // path, metadata, page count, open
    fn downloads(&self) -> &dyn DownloadSource;  // queue a download, read progress
    fn tankoubons(&self) -> &dyn TankoubonSource;// manifest for a repack
    fn activity(&self) -> &dyn ActivitySink;     // record share/import events
    fn settings(&self) -> &dyn SettingsSource;   // share + general settings reads
}
```

**Rationale**: a flat trait with 56 methods is a god trait; five focused traits composed by one host object keeps each
method group reviewable and lets the fake host implement only what a test needs.

## R3. Which types sink and where

| Type | Destination | Reason |
|---|---|---|
| `ArchiveId` and friends | `lanrurugi-core` | already a value type; both crates need it |
| record/`KnockV1`, ticket, claim payloads | `lanrurugi-share` (re-exported) | only the engine reads/writes them |
| share settings struct + validation | `lanrurugi-share` | the engine validates; the API stores |
| `ShareEvent`/activity payload | `lanrurugi-core` | activity log is owned by storage |
| archive metadata/path requests | `lanrurugi-core` | small DTOs, no API types |

**Rule**: no DTO may carry an `axum` type, a `Json` wrapper or an API error type across the boundary.

## R4. Background tasks keep their owner, count and order

**Decision**: the engine owns the tasks it owns today (endpoint watch, record refresh, relay serve, import drivers) and
exposes `start`/`stop`; `lanrurugi-server::main` calls `engine.start()` at the same point in the sequence it calls the
current `share_net::start()`. The API's own tasks (download queue, jobs) stay in the API and are reached through the
host.

**Rationale**: FR-010 forbids changing task count or ordering; moving the spawn site without moving the tasks would
change both.

## R5. The engine handle and the routers

**Decision**: routers get the handle (an `Arc` holding the engine and the host) exactly where they get `AppState` today;
handlers keep their request/response types and their auth policy in `lanrurugi-api`.

**Rationale**: moving handlers would break the route-scan docgen gate and put HTTP concerns in the engine.

## R6. Tests: fake host, not a fake application

**Decision**: sharing tests construct the engine with an in-memory host (archives served from a temp dir, settings
defaults, activity captured); the two-instance integration tests keep their real sockets and use a minimal host.

**Rationale**: the extraction is only real if a sharing test no longer needs `AppState`.

## R7. Dependency budget and cycle avoidance

**Decision**: `lanrurugi-share` depends on `lanrurugi-core`, `lanrurugi-storage`, `lanrurugi-share-proto` and the network
crates; it must not depend on `lanrurugi-api`, `lanrurugi-plugin`, `lanrurugi-llm` or the frontend. The build fails on a
cycle, which is the enforcement.

## R8. Mechanical first, interface second

**Decision**: the first engine commit is a file move plus `s/AppState/&dyn ShareHost/` at the boundary functions; no
logic rewrite. Behaviour changes, if any are discovered, go into their own commit and their own spec item.

**Rationale**: a move mixed with rewrites cannot be reviewed or bisected.
