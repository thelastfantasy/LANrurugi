---
description: "Task list for 019-recycle-bin-safety-password"
---

# Tasks: Recycle Bin and Safety Password

**Input**: design documents in `specs/019-recycle-bin-safety-password/`: plan.md, spec.md, research.md,
data-model.md, contracts/ (http-api.md, ui.md), quickstart.md.

**Tests**: the spec does not request TDD. Tests are included where a guarantee cannot be eyeballed: the
delete→restore round-trip, the scanner exclusion (a recycled file must never be re-catalogued), the lockout
arithmetic, token/guest denial, and the docgen route-coverage gate. Run them per crate with
`mise run test-crate -- <crate>`, detached per CLAUDE.md; the full `mise run test` and the visual UI sign-off
belong to the user.

**Organization**: phases follow spec priority. US1 is the MVP (delete + bin + restore). US2 and US3 are additive
and independently testable.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: can run in parallel (different files, no dependency on an unfinished task)
- **[Story]**: US1–US3 from spec.md

---

## Phase 1: Setup (Shared Infrastructure)

- [ ] T001 Add the storage types and repository skeleton in `crates/lanrurugi-storage/src/recycle.rs`: `RecycleEntry`,
  `RecycleSnapshot`, `RecycleActor`, `BookmarkSnapshot`, `RecycleEntryView`, `RecycleState` (as data-model.md),
  all `#[serde(rename_all = "snake_case")]`/`transparent` where applicable, and `RecycleRepository { pool }`.
  Register the module in `crates/lanrurugi-storage/src/lib.rs`.
- [ ] T002 [P] Add key constants to `crates/lanrurugi-storage/src/keys.rs`: `RECYCLE_BIN_KEY = "LANRURUGI_RECYCLE_BIN"`,
  `SAFETY_FAIL_KEY_PREFIX = "LANRURUGI_SAFETY_FAIL:"`, `SAFETY_LOCK_KEY_PREFIX = "LANRURUGI_SAFETY_LOCK:"`,
  and the `LRR_CONFIG` field names `safety_password_hash` / `safety_password_set_at` /
  `recycle_retention_days` / `recycle_max_size_mb` as constants.
- [ ] T003 [P] Add activity action types in `crates/lanrurugi-storage/src/activity.rs`'s `action_types`:
  `ARCHIVE_RECYCLE`, `ARCHIVE_RESTORE`, `ARCHIVE_PURGE`, `RECYCLE_EMPTY`, `RECYCLE_SWEEP`,
  `SAFETY_PASSWORD_SET`, `SAFETY_PASSWORD_RESET`, with one-line docs matching the neighbouring entries.
- [ ] T004 [P] Add the bin directory constant and path helpers in `crates/lanrurugi-scanner/src/watcher.rs`:
  `pub const RECYCLE_DIR_NAME: &str = ".lanrurugi-recycle";` and the component check inside
  `is_watched_archive_path` (before the extension check, next to the existing `thumb` check), so the watcher,
  `full_scan::walk`, `walk_subfolders` and `collect_archives_recursive` all exclude it from one place.
- [ ] T005 [P] Add a scanner regression test in `crates/lanrurugi-scanner/src/full_scan.rs` (its `#[cfg(test)]`
  module): a temp library containing `.lanrurugi-recycle/<id>/payload/book.zip` scans to
  `catalogued == 0`, while the same file outside the bin still catalogues.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: the repository, filesystem primitives and app wiring that every story uses.

- [ ] T006 Implement `RecycleRepository` in `crates/lanrurugi-storage/src/recycle.rs`: `put`, `get`, `list`
  (single `HGETALL`, newest first), `remove`, `usage()` (count + bytes), `all_ids()`. Deserialize failures must be
  logged and skipped, never fail the whole listing. Redis-gated unit tests mirroring the `download_queue.rs`
  test convention (skip when `LANRURUGI_TEST_REDIS_URL` is unset).
- [ ] T007 Implement the filesystem primitives in the same module: `bin_root(library)`, `entry_dir(library, id)`,
  `payload_dir(...)`, `write_meta_atomic` (temp file + rename), `read_meta`, `move_into_bin` (payload + optional
  patch, `rename`, `EXDEV` surfaced as an error not a copy), `move_out_of_bin`, `remove_entry_dir`. Unit tests
  with `tempfile` for a round-trip, for a missing payload, and for SC-006: on Unix, the payload's
  `MetadataExt::ino()` is unchanged after the move (a copy would allocate a new inode), so a future "helpful"
  copy fallback fails the test.
- [ ] T008 Wire `RecycleRepository` into `AppState`/`Repositories` in `crates/lanrurugi-api/src/state.rs`, on the
  `archive` pool, plus a `recycle_locks: Arc<tokio::sync::Mutex<HashSet<ArchiveId>>>`-backed per-entry guard with
  an RAII guard type (or a small `recycle::EntryLock` helper) implementing the "one mutation per entry" rule
  (FR-014).
- [ ] T009 Add `crates/lanrurugi-api/src/recycle.rs` with `pub fn router() -> Router<AppState>` registered in
  `crates/lanrurugi-api/src/lib.rs`, initially exposing only `GET /recycle-bin`; implement the listing handler
  returning `RecycleBinView` (entries + usage + limits + leftovers + `safety_password.configured`), with
  `restorable`/`conflict` derived at read time per data-model.md. `configured` is a direct `HEXISTS` on
  `LRR_CONFIG` here, deliberately not a call into the Phase 4 module, so Phase 2 never depends on US2.
- [ ] T010 Add the docgen sidecar `docs/api/sidecar/recycle-bin.yaml` covering every route added by this feature
  as it lands (start with `GET /api/recycle-bin`), following `docs/api/sidecar/sessions.yaml`'s shape, with
  `tags: [recycle-bin]` and shared error schemas. `mise run check-crate -- lanrurugi-api-docgen` must pass.

**Checkpoint**: the bin can be listed (empty), the scanner ignores it, the repository round-trips.

---

## Phase 3: User Story 1 — A deleted archive can be brought back (Priority: P1) 🎯 MVP

**Goal**: every user-initiated archive delete moves the file into the bin, removes the archive from every view,
and can be restored with full metadata and user data.

**Independent Test**: quickstart §1 and §2 — delete an archive carrying a title, tag, category, bookmark and
reading progress; confirm the bin entry; restore; verify everything is back and a rescan never re-catalogues it.

### Implementation

- [ ] T011 [US1] Snapshot builder in `crates/lanrurugi-api/src/recycle.rs`: read the `Archive`, then
  `categories.for_archive`, `bookmarks.list_for_archive`, `stamps.get` for each `stamp_ids` entry, and
  `archives.version_histories_for`; assemble `RecycleEntry` (including `deleted_by` from `AuthContext`, falling
  back to a `system` actor when `None`).
- [ ] T012 [US1] Split `delete_one_archive` in `crates/lanrurugi-api/src/archives.rs` into
  `purge_archive_record(state, id, archive, auth)` (today's cleanup tail: record, search index, recommend cache,
  categories, bookmarks, stamps, download queue — **without** thumbnail removal for the recycle path) and
  `recycle_archive(state, id, archive, auth)`: acquire entry + filename locks, move payload/patch into the bin,
  write `meta.json`, `RecycleRepository::put`, `HDEL LRR_FILEMAP[original_path]` (so a different file later
  arriving at that path is never re-keyed against the recycled id), then `purge_archive_record`, then activity.
  Any failure before the move leaves the archive untouched (FR-002); a failure after it rolls back best-effort.
- [ ] T013 [US1] Make the single and batch delete handlers return the additive field `recycled: true` (plus the
  batch `recycled_count`), keeping every existing field and the `DeleteOneOutcome` shapes otherwise intact, and
  add no filesystem path to the response (contracts/http-api.md).
- [ ] T014 [US1] Route every non-user deletion straight to `purge_archive_record` (FR-017): the missing-file
  Clean Database path (`crates/lanrurugi-api/src/database.rs`), the post-download superseded-revision sweep
  (`crates/lanrurugi-api/src/plugins.rs`, `delete_one_archive` with no auth), and `archive_split.rs`'s removal of
  the original after a successful split (today a bare `archives.delete` + `remove_file` at the end of `execute`).
  Update each call site's comment so the next refactor cannot route it through the bin; add one assertion per
  path that no `LANRURUGI_RECYCLE_BIN` field appears.
- [ ] T015 [US1] Keep thumbnails on recycle (do not unlink cover/per-page files in the recycle path) and verify no
  existing thumbnail cleanup treats the id as dead; add `GET /api/recycle-bin/{id}/thumbnail` in
  `crates/lanrurugi-api/src/recycle.rs` streaming the existing cover file with its content type, `404` when absent
  (FR-016).
- [ ] T016 [US1] Restore in `crates/lanrurugi-api/src/recycle.rs`: destination resolution per research §4
  (same-id conflict → `409 identical_archive_exists`; payload missing or size ≠ recording →
  `payload_missing`/`payload_size_mismatch`; free path → as-is; occupied → ` (restored N)`; missing
  directory/outside library → library root), then lock filename, move payload + patch out, `HSET
  LRR_FILEMAP[destination] = size_aware_id(payload)` (the scanner-computed id, **not** necessarily the stored id:
  equal for size-aware archives, and for a legacy-id entry writing the stored id would make the watcher re-key
  the record and orphan the stamps/bookmarks restore just re-created — see research §4), `archives.save`,
  re-apply categories/bookmarks/stamps/version history, remove the entry. Return
  `{operation, success, id, path, renamed}`. `POST /api/recycle-bin/{id}/restore` (FR-006–FR-009).
- [ ] T017 [US1] Startup reconciliation in `crates/lanrurugi-api/src/recycle.rs`: `reconcile(state)` over the
  bin directory vs the hash, applying the five-row table in research §3 (finish a half-done delete, rebuild
  missing `meta.json` from the record, adopt orphan entries, mark payload-less entries un-restorable, count
  leftovers with the 24 h grace). Call it once from `crates/lanrurugi-server/src/main.rs` before serving, and
  expose the leftover count in the listing (FR-013).
- [ ] T018 [US1] Activity entries for `archive.recycle` and `archive.restore` with actor, archive id, title and
  file path (FR-015), plus the matching labels in
  `apps/frontend/src/pages/Activity/OperationDescription.tsx` and `.../activityTarget.ts`.
- [ ] T019 [US1] Frontend data layer: `apps/frontend/src/api/recycle.ts` (types + fetchers) and TanStack Query
  hooks in `apps/frontend/src/api/hooks.ts` (`useRecycleBin`, `useRestoreArchive`), with cache invalidation of
  the library/search queries after a restore.
- [ ] T020 [US1] Frontend page `apps/frontend/src/pages/RecycleBin/`: `index.tsx` (page component only),
  `RecycleRow.tsx`, `RecycleBinToolbar.tsx` — thumbnail, title, filename, size, deleted-at, original path,
  restore action, un-restorable reason, empty state, filter input (contracts/ui.md). Route `/recycle-bin` in
  `apps/frontend/src/App.tsx` and the nav link in `apps/frontend/src/Layout.tsx`.
- [ ] T021 [US1] Delete-flow integration: extend the existing confirmation copy in
  `apps/frontend/src/pages/Library/ArchiveContextMenu.tsx` (and the Batch/Edit delete affordances) to say the
  archive moves to the bin, and add the "Moved to the recycle bin · Undo" toast action calling
  `useRestoreArchive` (FR-034). Add undo-action support to `apps/frontend/src/toast.tsx` if it has none.
- [ ] T022 [US1] Tests: API integration in `crates/lanrurugi-server/tests/` for delete→list→restore fidelity
  (title/tags/categories/bookmarks/stamps/progress/path, including the sidecar patch file and the `has_patch`
  flag), for the batch delete recycling every id, and for FR-004 after a delete: `/api/search`, the category's
  archive list, `/api/database/stats` and `/api/archives/{id}` all report the archive as absent; a
  `lanrurugi-storage` test for `rename`-based move + meta round-trip, and one restore test using a
  legacy-id entry (`legacy_id` fixture) asserting the record id, its stamps and its bookmarks survive a
  watcher-style re-ingest of the restored path. Token/guest denial is covered in T036.

**Checkpoint**: US1 is fully functional and is the shippable MVP.

---

## Phase 4: User Story 2 — Emptying the bin cannot be done by accident (Priority: P2)

**Goal**: a four-digit safety password gates permanent deletion and emptying, with lockout, two reset paths and
session-only authorization.

**Independent Test**: quickstart §3, §4 and §6.

### Implementation

- [ ] T023 [US2] `crates/lanrurugi-api/src/safety_password.rs`: credential read/write helpers over `LRR_CONFIG`
  (`hash_password`/`verify_password`, 4-ASCII-digit validation), excluded from the generic `GET/PUT /settings`
  payload (mirror `password`'s own exclusion in `settings.rs`).
- [ ] T024 [US2] Verification helper `verify_safety_password(state, auth, pin) -> Result<(), SafetyPasswordError>`
  implementing the lock-before-bcrypt order, the per-family counter/lock keys (TTL 300 s, threshold 5), the
  `safety_password_not_set` case and the generic `safety_password_incorrect` failure (FR-021–FR-024).
- [ ] T025 [US2] Endpoints in `safety_password.rs`: `GET /api/safety-password`, `POST /api/safety-password`
  (set/change; the current-password check goes through T024's helper so it shares the same counter and lock),
  `POST /api/safety-password/reset` (login password verified via `password::verify_password`, clears hash + lock
  keys). Every successful set/change/reset writes the `safety_password.set`/`safety_password.reset` activity
  entry with no PIN content (FR-036). Register the router (FR-018/FR-019/FR-025).
- [ ] T026 [US2] Purge and empty endpoints in `crates/lanrurugi-api/src/recycle.rs`: `DELETE /api/recycle-bin/{id}`
  and `DELETE /api/recycle-bin`, both taking `{safety_password}`, resolving in the contract's fixed order
  (session → password/lock → entry → per-entry lock → execute) so a wrong PIN never reveals whether the id
  exists, then removing payload, patch, thumbnails, hash field and `meta.json`; partial failures reported per id
  and surviving entries kept (FR-010, FR-011, FR-022). Record `archive.purge` / `recycle.empty` activity.
- [ ] T027 [US2] Lockout, per-entry serialization and conflict responses on the three mutations: `429` with
  `locked_until` while locked (checked before the entry lookup), `404` only once verification has passed, `409 in
  progress` / `404 already gone` for a lost race (FR-014, FR-022).
- [ ] T028 [US2] Authorization: add explicit `token_admin`/`token_guest` deny lines for `/api/recycle-bin*` and
  `/api/safety-password*` to `crates/lanrurugi-api/policy/route_policy.csv`; add the matching assertions to
  `crates/lanrurugi-api/src/authz.rs`'s test block (FR-027).
- [ ] T029 [US2] CLI: add `ResetSafetyPassword { redis_url }` to `crates/lanrurugi-server/src/main.rs`, clearing
  the two `LRR_CONFIG` fields and every `LANRURUGI_SAFETY_*` key, printing one confirmation line and never
  accepting/printing a password (FR-026). Document it in the binary's `--help` text and `README.md`'s CLI list.
- [ ] T030 [US2] Create the shared component `apps/frontend/src/components/common-ui/Form/OTPField.tsx`,
  built on Base UI's `OTPField` (`@base-ui/react/otp-field`: `Root` / `Input` / `Separator`, already in the
  installed `@base-ui/react`): props `length` (default 4), controlled `value` + `onValueChange`, `onComplete`,
  `mask` (default off so the four digits stay readable while typing), `disabled`, `invalid`, `autoFocus`,
  `label`, `slotLabel(index, total)` for the per-slot `aria-label`, and `describedById`. Root renders with the
  `.otp-field` class and `data-invalid`; every slot is `OTPField.Input` with `.otp-field-slot`; on completion,
  focus moves to the caller's confirm button rather than auto-submitting. Export it from `Form/index.ts` and the
  common-ui barrel. Vitest/RTL test (`OTPField.test.tsx`): four slots render, typing/pasting/backspace fill and
  clear them, `onValueComplete` fires once at four digits, disabled/invalid attributes propagate, and only the
  first slot carries the label (the rest get the position `aria-label`).
- [ ] T031 [P] [US2] Add the OTP field's theme classes to **all five** files in
  `apps/frontend/public/legacy/themes/` (`g.css`, `ex.css`, `modern.css`, `modern_red.css`,
  `modern_clear.css`), per CLAUDE.md's custom-colour rule and the `.tankoubon-member-row` precedent: `.otp-field`
  (flex layout + gap), `.otp-field-slot` (box size, border, background, text colour — each taken from that
  theme's existing input/field colours), `.otp-field-slot[data-filled]`, `.otp-field-slot:focus-visible` (that
  theme's accent hue, the same one its `.tankoubon-member-row` uses), `.otp-field[data-invalid] .otp-field-slot`
  (that theme's error/danger colour), `.otp-field-separator`, and a `@media (max-width: 560px)` size step.
  Append each file's block at its end with a one-line comment; do not use the transitional `--theme-*` variables
  from `apps/frontend/src/index.css`, and do not put any of these values in an inline `style`.

- [ ] T032 [US2] Frontend: `apps/frontend/src/pages/RecycleBin/SafetyPasswordDialog.tsx` implementing
  contracts/ui.md (setup-when-unconfigured, the shared `OTPField` for every four-digit entry, attempts
  remaining, lock countdown, focus trap/Escape/focus return), plus `useSafetyPasswordStatus` /
  `useSetSafetyPassword` / `useResetSafetyPassword` / `usePurgeEntry` / `useEmptyBin` hooks and the row/toolbar
  wiring.
- [ ] T033 [US2] Frontend: safety-password block (set/change/reset) in
  `apps/frontend/src/pages/Settings/RecycleBinSection.tsx`, reusing `OTPField` (never a second PIN input) and
  the same submit/error handling as the dialog.
- [ ] T034 [US2] Tests: API integration for not-configured refusal, wrong PIN changes nothing, five failures lock
  (correct PIN refused while locked; the counter is shared with the change-current-password check), lock expiry
  via a manually deleted lock key, reset with the login password, a wrong PIN on a nonexistent id answering the
  same way as on an existing one, the `safety_password.*` activity entries carrying no PIN content, and no
  response containing the hash or the submitted digits. Unit tests for the 4-digit validator.
- [ ] T035 [US2] Frontend unit tests (Vitest) for the dialog's submit gating (four digits required, mismatch when
  confirming a new password), the attempts-remaining copy and the lock countdown formatting — slot mechanics
  themselves are covered by T030's own component test, not duplicated here.
- [ ] T036 [US2] Authorization integration tests: an admin token gets `403` from every recycle/safety endpoint, a
  guest visitor gets `403`/`404`, and a session gets `200` (complements T028's Casbin-level test).

**Checkpoint**: US1 + US2 work; an accidental empty is no longer possible.

---

## Phase 5: User Story 3 — The bin stays bounded (Priority: P3)

**Goal**: configurable retention and size cap, enforced automatically, with usage and remaining time visible.

**Independent Test**: quickstart §5.

### Implementation

- [ ] T037 [US3] Add `recycle_retention_days` (default 30, `0..=36500`) and `recycle_max_size_mb`
  (default 10240, `0..=1_048_576`) to `NUMBER_FIELDS` in `crates/lanrurugi-api/src/settings.rs`, with the
  range check in `validate_setting_field` and the two values included in the settings GET payload.
- [ ] T038 [US3] `sweep(state)` in `crates/lanrurugi-api/src/recycle.rs`: delete entries past retention (skip
  when 0), then evict oldest-first until within the cap but never the newest entry (skip when 0); each removal
  deletes payload/patch/thumbnails/entry and writes `recycle.sweep` activity with `System` actor and
  `reason: expired|over_capacity` (FR-028–FR-031).
- [ ] T039 [US3] Scheduling: hourly `tokio::time::interval` task in `crates/lanrurugi-server/src/main.rs`
  alongside the existing sweeps, one run at startup after `reconcile`, and an immediate spawn after a
  `PUT /settings` that lowers either bound (T037's hook). Never block request handling (FR-030).
- [ ] T040 [US3] Immediate enforcement hook in `crates/lanrurugi-api/src/settings.rs`: after a successful write
  that changes either bound, spawn `recycle::sweep`, so tightening takes effect without waiting for the hour.
- [ ] T041 [US3] Listing additions: `expires_at`/`remaining_seconds` per entry, `leftovers`, and the usage/limits
  block already scaffolded in T009, now driven by the real settings (FR-031/FR-032).
- [ ] T042 [US3] Frontend: `RecycleBinSection.tsx` retention/capacity inputs + usage line + effective-limit text,
  saved through the existing settings form state in `SettingsPage.tsx`; the bin page shows remaining time per row
  and a usage header ("N entries · X of Y") (FR-032/FR-033).
- [ ] T043 [US3] Tests: storage/API tests for expiry ordering, cap eviction order, the newest-entry carve-out,
  retention `0` meaning forever, cap `0` meaning unlimited, and a settings-tightening sweep triggered without a
  restart.

**Checkpoint**: all three stories work; the bin cannot grow without bound.

---

## Phase 6: Polish & Cross-Cutting Concerns

- [ ] T044 Complete `docs/api/sidecar/recycle-bin.yaml` for every route and run
  `mise run check-crate -- lanrurugi-api-docgen`; the route-scan gate must show no undocumented route (011).
- [ ] T045 i18n: add every new string to all 14 files under `apps/frontend/src/i18n/locales/` (English source
  keys; zh/zh_Hant/ja at minimum reviewed, others falling back to English) (FR-035).
- [ ] T046 Playwright coverage in `apps/frontend/tests/e2e/`: delete → undo → restore; set PIN → permanent delete;
  empty with wrong PIN → lockout message; un-restorable row rendering.
- [ ] T047 Activity page: render the seven new action types (`archive.recycle`/`restore`/`purge`,
  `recycle.empty`/`sweep`, `safety_password.set`/`reset`) with human labels and icons, and verify `recycle.sweep`
  shows a `System` actor.
- [ ] T048 Docs: update `README.md`'s feature bullets (recycle bin + safety password) and
  `docs/comparison-with-lanraragi.md` if it claims deletion is permanent; record the CLI subcommand. Run the
  CLAUDE.md "before pushing" README check explicitly.
- [ ] T049 Run quickstart §1–§7 against a real instance, then `mise run check` (rust-check + frontend-lint) and
  the detached `mise run test`; fill in the Implementation Record below.

---

## Dependencies & Execution Order

- **Phase 1** has no dependencies. T002–T005 are parallel; T001 first (it defines the types the rest use).
- **Phase 2** depends on Phase 1: T006 after T001, T007 after T001/T004, T008 after T006, T009 after T006–T008,
  T010 can start once T009's route exists (and grows with every later route).
- **US1 (Phase 3)** depends on Phase 2 and is the MVP. T011 → T012 (snapshot before the move) → T013/T014/T015 →
  T016 → T017 → T018; frontend T019–T021 can start after T016; T022 last.
- **US2 (Phase 4)** depends on T006/T009 for the entry operations and on T012 for the purge path. T023 → T024 →
  T025/T026/T027 → T028 → T030/T031 → T032 → T033 → T034–T036.
- **US3 (Phase 5)** depends on T037 (settings) and T038 (sweep) → T039/T040 → T041/T042 → T043.
- Within US2, T029 (CLI) is independent of the UI tasks and can be done any time after T023.
- **Polish** T044–T047 can run in parallel once their stories land; T048/T049 last.

## Parallel Examples

- Setup: T002 (keys), T003 (activity types), T004 (scanner exclusion), T005 (scanner test) touch four different
  files and can run together.
- US1 frontend: T019 (api/hooks), T020 (page), T021 (delete flow) touch disjoint files once the DTOs in T016 are
  fixed.
- US2: T025 (endpoints), T028 (policy), T029 (CLI), T030 (OTP component), T031 (theme CSS) and T032 (dialog) are
  six separate files.
- Polish: T044 (sidecar), T045 (locales), T046 (e2e), T047 (activity labels) are independent.

## Implementation Strategy

**MVP**: Phases 1–3 only. A user can delete and restore archives with full metadata; the bin is unbounded and
unguarded, which is still strictly better than today's irreversible delete. Validate with quickstart §1–§2 and
the T022 tests before starting US2.

**Then**: US2 (the safety net's lock) and US3 (the bounds), each validated by its own quickstart section and
tests. No story requires the previous one to be refactored.

## Implementation Record

_Fill in during `/speckit-implement`: what was verified against a real instance (which quickstart sections, which
crates' tests), any departure from plan.md/research.md, and anything left for the user to verify (full test run,
visual UI sign-off)._
