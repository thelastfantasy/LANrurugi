# Tasks: Share Crate Extraction

**Input**: Design documents from `/specs/023-share-crate-extraction/`
**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Existing tests move with their code and must pass unedited in meaning; a test that needs application data
uses the test host from T008, never `AppState`.

**Organization**: four commits as research.md decided — type sinks, engine move, host interface, tests/scripts. Each
commit must leave the workspace buildable and the observable behaviour unchanged.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel
- **[Story]**: US1–US4 as in spec.md

---

## Phase 1: Setup

- [ ] T001 Create `crates/lanrurugi-share/Cargo.toml` with the dependencies the engine actually uses today
  (iroh, iroh-relay client, tokio, serde, postcard, tracing, …), **not** `axum`/`lanrurugi-api`; add it to the
  workspace members. Verify `cargo tree -p lanrurugi-share | grep lanrurugi-api` is empty at every later commit.
- [ ] T002 [P] Capture the baseline: `cargo tree` for the crates involved, `mise run docs-api -- --check`, and a
  `scripts/share-network-matrix.sh` run recorded in this file's implementation record (before any move).

---

## Phase 2: Foundational — type sinks (commit 1)

**Purpose**: a type both sides need must live below both; duplicating it would create two definitions that drift.

- [ ] T003 Move the boundary types from `crates/lanrurugi-api/src/share_net/` (record/ticket/knock/claims/permission
  DTOs) into `crates/lanrurugi-share-proto` or the new crate per research.md's table; re-export them from
  `lanrurugi-api` so no handler changes in this commit.
- [ ] T004 Move `ArchiveId`-adjacent helpers and the share settings struct out of `share_settings.rs` into the new
  crate (or `lanrurugi-core` where they are not sharing-specific), keeping the setting keys and defaults identical.
- [ ] T005 [P] Move activity/logging calls behind a thin `ActivitySink` trait in the new crate with the API side
  implementing it over `crate::activity::record_manual`.
- [ ] T006 Tests for the sinks: type round-trips unchanged (postcard bytes identical for a known record), settings
  defaults identical to `defaults_fill_unset_fields`'s expectations.

---

## Phase 3: User Story 1 & 3 — Engine move, nothing observable changes (Priority: P1, commit 2)

**Goal**: `share_net/*` lives in `crates/lanrurugi-share`; the workspace builds and every test passes with the wire
format, keys, records and settings unchanged.

- [ ] T007 Move `crates/lanrurugi-api/src/share_net/` into `crates/lanrurugi-share/src/` mechanically (git mv, then
  fix paths), module by module, keeping each module's tests next to it. No logic edits in this commit.
- [ ] T008 Replace the moved code's `AppState` uses (56 call sites at baseline) with the host trait from Phase 4
  *only where the compiler forces it*; where a narrow local handle suffices, pass that instead.
- [ ] T009 Keep `crates/lanrurugi-api/src/share_recipient.rs`'s pure helpers in the new crate if they do not touch
  HTTP; anything that reads a request or writes a response stays in `lanrurugi-api`.
- [ ] T010 Run the existing tests (`test-crate -- lanrurugi-share lanrurugi-api`) and `docs-api -- --check`; the
  route table must be byte-identical to the baseline.

---

## Phase 4: User Story 2 — One explicit application boundary (Priority: P1, commit 3)

**Goal**: the engine asks `ShareHost` for archives, downloads, tankoubons, activity and settings; no `AppState`, no
`lanrurugi_api` import.

- [ ] T011 Define `pub trait ShareHost: Send + Sync + 'static` with the five grouped accessors from
  `contracts/host.md` (`archives()`, `downloads()`, `tankoubons()`, `activity()`, `settings()`) and their sub-traits;
  every method takes/returns owned or `&`-borrowed plain types, never an API type.
- [ ] T012 Implement the trait in `crates/lanrurugi-api/src/share_host.rs` over `AppState`, one `impl` block per
  accessor, with `ShareError::Host` mapping failures opaquely (no API error types crossing the boundary).
- [ ] T013 Change `ShareEngine::new(host)` and the background-task spawns to hold `Arc<dyn ShareHost>`; update
  `crates/lanrurugi-server/src/main.rs`'s construction and shutdown order (T014 keeps it identical).
- [ ] T014 Add the test host (in-memory archives/downloads/tankoubons/activity/settings) in the crate's `tests/` or
  a `test_support` module behind `cfg(any(test, feature = "test-support"))`; port the sharing tests that used
  `AppState` fixtures onto it.
- [ ] T014a [US1] Document the crate's public surface: crate-level docs on what the engine owns vs. what the host
  provides, a doc comment on every `ShareHost` method and every public type, and `cargo doc -p lanrurugi-share
  --no-deps` with `RUSTDOCFLAGS="-D warnings"` clean.
- [ ] T014b [US3] Assert FR-010: `git diff` the startup log (task names, order, count) against T002's baseline; a
  changed task set or order is a finding, not an acceptable side effect.

- [ ] T015 Grep gates: `AppState|lanrurugi_api` must be absent from `crates/lanrurugi-share/`; add a CI check for it
  so the boundary cannot decay.

---

## Phase 5: User Story 4 — HTTP stays put (Priority: P2, commit 4)

- [ ] T016 Verify every share route still lives in `lanrurugi-api` (`/api/shares`, `/api/share/imports`,
  `/api/share/r/{token}/…`, `/api/share/settings`, `/api/share/status`, relay/knock routes) and that the handlers
  only call the engine; no handler moved into the new crate.
- [ ] T017 Re-run `mise run docs-api -- --check`; the route scan must still find every share route (the docgen parses
  `crates/lanrurugi-api/src/**`, so a route that moved to the new crate would silently vanish from the docs).
- [ ] T018 Run `scripts/share-network-matrix.sh` and record that it is unchanged from T002's baseline.

---

## Phase 6: Polish

- [ ] T019 [P] Update `README.md`/`CONTRIBUTING.md` if they name the crate layout, and `docs/` architecture notes if
  they describe `share_net` living in the API crate.
- [ ] T020 Gates: `mise run fmt-check`, `mise run clippy`, `mise run check-crate -- lanrurugi-share lanrurugi-api`,
  `mise run test-crate -- lanrurugi-share lanrurugi-api`, `cargo tree -p lanrurugi-share | grep -c lanrurugi-api`
  must print 0, and `mise run docs-api -- --check`.

---

## Dependencies & Execution Order

- T001 before everything; T002 is the baseline for every later comparison.
- Phase 2 (T003–T006) before Phase 3 (T007) — the moved code must compile against the sunk types.
- Phase 3 must be a pure move; if a logic edit is required to compile, it belongs in Phase 4.
- Phase 4 (T011–T015) before Phase 5's verification, and after Phase 3's build is green.
- T014 (test host) blocks porting any test that used `AppState`.
- T020 last.

## Parallel Opportunities

T003/T004/T005 (different type groups), T010 with T011's trait sketch, T019 with T018.

## Implementation Strategy

Four reviewable commits, each green: (1) types sink, (2) mechanical move with `AppState` still reachable through a
shim if the compiler demands it, (3) host trait and its API implementation, (4) tests ported and gates. Rebase
`vendor/` changes from 022 as needed — 023 must land after 022 so the engine is not moved twice.

## Notes

- This is a refactor: any behavioural diff is a bug. If a test needs a semantic edit, stop and inspect — the test
  was probably relying on an `AppState` detail the host must expose explicitly.
- `share_net/`'s background tasks are owned by the engine but scheduled by `main`; keep that split.
