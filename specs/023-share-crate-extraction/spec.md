# Feature Specification: Share Crate Extraction

**Feature Branch**: `023-share-crate-extraction`

**Created**: 2026-11-08

**Status**: Draft

**Input**: Review `share-pr-review.md` §0 — "共享拆独立 crate" was a stated goal of the sharing work and was not done: `share_net/` (~7,100 lines), `shares.rs`, `share_imports.rs`, `share_recipient.rs` and `share_settings.rs` (~10,700 lines together) still live in `lanrurugi-api`, and `share_net/` reaches into `AppState` in 56 places. The user's decision (2026-11-08) is that this is required, not optional.

## Context

The sharing feature was built inside the HTTP crate because that is where the routers, the repositories and the settings live, but it is a network service with its own lifecycle: it starts before any request, it survives router rebuilds, it owns an iroh endpoint and a relay, and its failures are not HTTP responses. Keeping it in `lanrurugi-api` has three costs:

- **Coupling**: `share_net` reaches into `AppState` for downloads, archives, activity, settings and repositories. A change to any of those can break a background task with no compile-time signal, and the sharing code cannot be tested without constructing a full API state.
- **Build and review weight**: the crate that holds every HTTP handler is also the crate that holds a ~7,100-line P2P stack; a sharing-only change rebuilds the largest crate in the workspace, and a reviewer of HTTP changes scrolls past the whole stack.
- **Boundary erosion**: because everything is reachable, new sharing code keeps reaching further in. The extraction is the forcing function for a narrow, explicit interface.

The extraction is a pure refactor: no protocol change, no behaviour change, no new feature. The only intentional change is the shape of the code and the trait through which the share stack asks the rest of the application for what it needs.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Sharing code builds and tests without the HTTP crate (Priority: P1)

A maintainer changes the sharing stack. They run the share crate's own tests and clippy without building the HTTP crate or its router, and the workspace still compiles.

**Why this priority**: it is the reason to extract; if the crate still drags `lanrurugi-api` in, nothing was gained.

**Independent Test**: `cargo tree -p lanrurugi-share` (or the workspace's check task for that crate) shows no dependency on `lanrurugi-api`, and `mise run check-crate -- lanrurugi-share` passes.

**Acceptance Scenarios**:

1. **Given** the extracted crate, **When** its dependency tree is inspected, **Then** it does not depend on `lanrurugi-api`.
2. **Given** a change to the sharing stack only, **When** the workspace is built, **Then** the API crate is not recompiled from scratch for a sharing-only edit beyond linking.

---

### User Story 2 - The application boundary is one explicit interface (Priority: P1)

The share stack needs a download, an archive path, a tankoubon manifest, activity logging, settings and repositories. Instead of reaching into `AppState`, it asks a `ShareHost` interface implemented by the API layer.

**Why this priority**: this is the property that keeps the extraction from decaying back into `AppState` access.

**Independent Test**: grep the new crate for `AppState` and for `lanrurugi_api` — both must be absent.

**Acceptance Scenarios**:

1. **Given** the extracted crate, **When** it needs data owned by the API, **Then** it calls a method on the host trait.
2. **Given** a new sharing need, **When** it is added, **Then** it is added to the trait with a documented contract, not by importing an API type.

---

### User Story 3 - Nothing observable changes (Priority: P1)

An operator upgrades. Import, export, sharing, relays, ports, settings and the share pages behave exactly as before; the network matrix still runs; no endpoint, key or record format changed.

**Why this priority**: a refactor that changes behaviour is a regression with extra steps.

**Independent Test**: run the existing test suite and `scripts/share-network-matrix.sh` before and after; the results must match.

**Acceptance Scenarios**:

1. **Given** the same two instances, **When** an import runs across the change, **Then** the wire protocol, records and settings keys are unchanged.
2. **Given** the existing tests, **When** they run after the move, **Then** they pass without semantic edits.

---

### User Story 4 - HTTP stays in the HTTP crate (Priority: P2)

The API crate keeps the share routers, handlers, extractors and auth policy; the new crate owns the engine and the state, not the routes.

**Why this priority**: moving handlers into the share crate would trade one coupling for another and break the route-scan documentation gate.

**Independent Test**: `lanrurugi-api`'s router still registers the share routes and `mise run docs-api -- --check` still passes.

**Acceptance Scenarios**:

1. **Given** the extracted crate, **When** the route table is generated, **Then** every share route is still served and documented.
2. **Given** an HTTP-layer change to a share route, **When** it is made, **Then** it does not require touching the engine crate.

---

### Edge Cases

- A type used on both sides of the boundary (an archive id, a permission set, a settings struct) is moved to the storage/core crate rather than duplicated.
- `ShareHost` methods that are only needed by one caller are kept narrow; a "god trait" is a failure of the extraction.
- The extraction must not create a dependency cycle (`share` → `api` → `share`); the workspace build catches it.
- Background tasks that today capture `AppState` must capture the host trait object or a narrower handle.
- The share crate must not gain a dependency on the frontend build, the plugin pool or any HTTP framework it does not already use.
- Test fixtures that construct `AppState` for sharing tests must be expressible with a host implementation instead.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: A new workspace crate (proposed name `lanrurugi-share`) MUST own the sharing engine: the network stack, node/relay lifecycle, import/export state machines, share settings and share-recipient helpers.
- **FR-002**: The crate MUST NOT depend on `lanrurugi-api`, directly or transitively.
- **FR-003**: All needs the engine has on the rest of the application MUST be expressed through one interface implemented by the API layer.
- **FR-004**: HTTP routing, extractors, auth policy and response shaping MUST remain in `lanrurugi-api`.
- **FR-005**: Wire formats, record formats, settings keys, endpoints and persisted state MUST NOT change.
- **FR-006**: The extraction MUST NOT introduce a dependency cycle and MUST keep the workspace buildable at every commit.
- **FR-007**: Existing tests MUST move with the code they test; tests that need a host implementation MUST use a test host, not a full API state.
- **FR-008**: The public items of the new crate MUST be documented enough that the API layer needs no knowledge of internals.
- **FR-009**: The route-scan documentation gate MUST continue to see every share route.
- **FR-010**: The extraction MUST NOT change the number or nature of background tasks, nor their start/stop ordering relative to the HTTP server.
- **FR-011**: The network matrix script MUST run unchanged against the extracted build.
- **FR-012**: Any type shared across the boundary that is not already in a lower-level crate MUST move to the lowest crate that both can depend on.

### Key Entities

- **Share engine**: the stateful service (endpoint, relay, imports, exports, records) extracted into its own crate.
- **Host interface**: the narrow trait through which the engine requests archives, downloads, activity and settings from the application.
- **HTTP layer**: the routers and handlers that remain in `lanrurugi-api`.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: `cargo tree -p lanrurugi-share` shows no path to `lanrurugi-api`.
- **SC-002**: `lanrurugi-api` loses at least 9,000 lines of sharing code to the new crate.
- **SC-003**: The full test suite and the network matrix pass with identical results before and after.
- **SC-004**: A sharing-only change no longer requires rebuilding a crate that contains the HTTP router.
- **SC-005**: `docs-api --check` still passes and lists the same share routes.
- **SC-006**: The host trait has no method that exists for a single unstructured "give me everything" call.

## Assumptions

- The extraction is mechanical first and architectural second: move, make it compile, then narrow the boundary where it is cheap.
- The storage crate already owns the repositories and ids the engine needs; only application-level orchestration needs the host trait.
- No new external dependency is required by the extraction.
- The engine keeps its current task model (long-lived tasks owned by one supervisor), not a new actor framework.
- The existing `lanrurugi-egress` and `lanrurugi-share-proto` crates stay where they are; they are already separate.
