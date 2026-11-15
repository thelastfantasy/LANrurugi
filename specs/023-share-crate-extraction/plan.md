# Implementation Plan: Share Crate Extraction

**Branch**: `023-share-crate-extraction` | **Date**: 2026-11-08 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/023-share-crate-extraction/spec.md`

## Summary

Move the sharing engine out of `lanrurugi-api` into a new `crates/lanrurugi-share`, and make every need it has on the
application go through one `ShareHost` interface implemented by the API layer. HTTP routing, extractors, auth policy and
response shaping stay in `lanrurugi-api`.

The move is mechanical and bottom-up, so the workspace is green after every step:

1. **Leaves first**: types both sides need (archive id, permission set, share settings struct, record types) move to the
   lowest crate that can own them (`lanrurugi-core`/`lanrurugi-storage`), with re-exports in `lanrurugi-api` for one
   commit so no caller changes in the same step.
2. **The engine**: `share_net/` (endpoint, transports, relay, nodes, publish/claim, import/export state machines) moves
   into `lanrurugi-share`; its `AppState` parameters become calls on `ShareHost`.
3. **The adapter**: `lanrurugi-api` implements `ShareHost` over the existing repositories, download queue, activity log
   and settings store; the share routers keep calling the engine handle exactly as the state does today.
4. **Tests and scripts**: tests move with the code, sharing tests get a fake host instead of an `AppState`; the network
   matrix script and the route-scan docgen gate run unchanged.

## Technical Context

**Language/Version**: Rust 2021 workspace

**Primary Dependencies (new crate)**: `iroh`, `iroh-relay`, `netwatch` (patched by 022), `tokio`, `serde`, `postcard`,
`pkarr`, `reqwest`, `lanrurugi-core`, `lanrurugi-storage`, `lanrurugi-share-proto`; **not** `axum`, **not**
`lanrurugi-api`

**Storage**: the same Redis keys and the same on-disk state, unchanged; the engine is given a `RedisPool`/repository
handles through the host

**Testing**: `mise run check-crate -- lanrurugi-share`, per-crate tests with a fake host, the existing two-instance
integration suite (adapted to construct the engine with a host), `scripts/share-network-matrix.sh`

**Target Platform**: same as the server (Linux/FreeBSD/macOS)

**Project Type**: workspace refactor

**Performance Goals**: no measurable change; the trait calls are static dispatch or `dyn` at the start of a request, not
per chunk

**Constraints**: workspace green at every commit; wire/record/settings unchanged; no cycle; route docgen unaffected;
background task count and start order unchanged

**Scale/Scope**: ~10,700 lines of sharing code, 56 `AppState` uses, five files (`share_net/`, `shares.rs`,
`share_imports.rs`, `share_recipient.rs`, `share_settings.rs`)

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Status | How this plan satisfies it |
|---|---|---|
| I. Legacy data & user-trust compatibility | PASS | No record, key, path or setting changes; the same bytes are read and written by the same code, compiled into a different crate. |
| II. API contract fidelity | PASS | Routes and handlers do not move; the route table and OpenAPI output are byte-identical. |
| III. Single-process, resource-conscious | PASS | No new process, no new task, no extra hop; the engine keeps its own tasks and its existing budgets. |
| IV. Plugin extensibility | PASS | The plugin runtime is not touched; no plugin-facing type moves. |
| V. Secrets & network trust boundaries | PASS | Secrets handling moves unchanged; the host trait exposes data, not credentials. |
| VI. Phased scope discipline | PASS | Pure Phase 1 refactor. |
| VII. Frontend discipline | PASS | No frontend change. |
| Tech stack: workspace, newtype IDs | PASS | One new workspace member; IDs keep their newtypes. |

**Complexity Tracking**: one new crate and one interface. The alternative — leaving the code in the HTTP crate — keeps a
7,100-line P2P stack inside the crate that rebuilds for every handler change and keeps the 56 `AppState` edges, which
is the problem the spec exists to remove.

## Project Structure

### Documentation (this feature)

```text
specs/023-share-crate-extraction/
├── plan.md
├── research.md
├── data-model.md
├── contracts/host.md
├── quickstart.md
└── checklists/requirements.md
```

### Source Code (repository root)

```text
crates/lanrurugi-share/            # NEW
├── src/lib.rs                     # public handle: start/stop/status/rotation
├── src/host.rs                    # the ShareHost trait (single boundary)
├── src/engine.rs                  # lifecycle, wiring, task ownership
├── src/net/                       # moved from share_net/: endpoint, transports, server, relay
├── src/nodes.rs                   # records, publish, allow-list
├── src/import.rs                  # import state machine
├── src/export.rs                  # share creation/claim/ticket
├── src/settings.rs                # share settings struct and validation
└── src/recipient.rs               # recipient-side helpers (page base, download base)

crates/lanrurugi-api/src/
├── share_net/                     # REMOVED (thin re-export during the transition only)
├── shares.rs                      # routers/handlers, calls engine handle
├── share_imports.rs               # routers/handlers
├── share_recipient.rs             # routers/handlers
├── share_settings.rs              # handlers over lanrurugi-share's settings type
└── share_host.rs                  # NEW: impl ShareHost over repos/queue/activity/settings
```

**Structure Decision**: engine in one crate, HTTP in the other, boundary in one trait. `lanrurugi-share` may depend on
`lanrurugi-core`/`lanrurugi-storage`; it must never depend on `lanrurugi-api`.

## Complexity Tracking

| Violation | Why Needed | Simpler Alternative Rejected Because |
|---|---|---|
| One new workspace crate | The spec's whole point: the P2P stack must build and test without the HTTP crate | Keeping it in `lanrurugi-api` rebuilds the largest crate for sharing edits and leaves the 56 `AppState` edges |
| One `ShareHost` trait | The engine needs archives, downloads, activity and settings from the app | Passing `AppState` (impossible across the crate boundary) or duplicating the data in the engine |

## Phase 0 — Research

See [research.md](./research.md): move order, what belongs in the host trait, which types sink to core/storage, how
background tasks and start order stay the same, and how tests and the matrix script survive the move.

## Phase 1 — Design

See [data-model.md](./data-model.md) for the crate/module map and ownership, [contracts/host.md](./contracts/host.md)
for the trait, and [quickstart.md](./quickstart.md) for the verification sequence.

## Post-Design Constitution Re-check

Unchanged; the refactor adds a crate and a trait, removes 56 implicit dependencies, and changes no observable behaviour.
