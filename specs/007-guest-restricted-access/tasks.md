---

description: "Task list for Restricted Guest Access Mode"
---

# Tasks: Restricted Guest Access Mode

**Input**: Design documents from `/specs/007-guest-restricted-access/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/guest-access-api.md,
quickstart.md (all present)

**Tests**: Included — spec's Success Criteria (SC-002/SC-003 in particular) are security
guarantees ("100% of archives outside scope unreachable", "100% of administrative functions
reject unauthenticated access"), which are not credible without automated regression coverage, and
research.md §8 already commits to a specific list of new/updated/deleted tests.

**Organization**: Tasks are grouped by user story per spec.md's three stories (US1 mandatory
admin password, US2 guest-mode opt-in browsing, US3 guest capability boundary), each
independently completable/testable per its own Independent Test description in spec.md.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (US1, US2, US3)
- Every task includes exact file paths (per plan.md's Project Structure)

## Path Conventions

Web app (per plan.md Project Structure): `crates/*/src/`, `crates/lanrurugi-server/tests/` for the
Rust backend; `apps/frontend/src/`, `apps/frontend/tests/unit/`, `apps/frontend/tests/e2e/` for the
React frontend.

---

## Phase 1: Setup

**Purpose**: No project-initialization work is needed — this feature extends the existing Phase 1
workspace/app (plan.md Structure Decision: no new crate, no new deployable). This phase is
intentionally empty; proceed directly to Phase 2.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Data-model and auth-primitive changes every user story depends on. None of the three
user stories can be implemented until this phase is complete — US1 needs the auth-context/Casbin
groundwork, US2 needs the Category field and Casbin policy, US3 needs the search-engine scoping
primitive.

**⚠️ CRITICAL**: No user story work can begin until this phase is complete.

- [X] T001 Add `visible_to_guest: bool` field to `Category` struct in
  `crates/lanrurugi-core/src/entities.rs` (entities.rs:118-135)
- [X] T002 [P] Read/write `visible_to_guest` in `CategoryRepository::get`/`save` in
  `crates/lanrurugi-storage/src/repository.rs` (repository.rs:310-316 read, 396-404 write),
  copying `pinned`'s exact `unwrap_or(false)` absent-default pattern (data-model.md §Category)
- [X] T003 [P] Add `restrict_to_archive_ids: Option<HashSet<ArchiveId>>` field to `SearchParams` in
  `crates/lanrurugi-search/src/engine.rs` (engine.rs:29-50), applied via the same
  `HashSet::retain` pattern as `category`/`untaggedonly`/`tankonly` (engine.rs:107-134;
  research.md §4)
- [X] T004 Add `AuthMethod::GuestVisitor` variant (no payload) to
  `crates/lanrurugi-api/src/auth_context.rs` (auth_context.rs:10-19; research.md §1)
- [X] T005 Add `"guest_visitor"` match arm to `authz::subject_role` in
  `crates/lanrurugi-api/src/authz.rs` (authz.rs:49-62; research.md §2)
- [X] T006 Add the `guest_visitor` Casbin policy block to
  `crates/lanrurugi-api/policy/route_policy.csv` — GET-only allow, `token_guest`'s existing
  15-entry deny list reused verbatim, plus the `guest_visitor`-specific
  `/api/archives/:id/download` (GET) deny (research.md §3); remove the entire `anonymous` subject
  block (all allow + deny lines) in the same file
- [X] T007 Add `guestmode: bool` to `BOOL_FIELDS` and remove `enablepass`/`nofunmode`/`devmode`
  from it in `crates/lanrurugi-api/src/settings.rs` (settings.rs:224-236); remove
  `enablepass`/`nofunmode` from `TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS` (settings.rs:365-370)
- [X] T008 Swap the `devmode` example field to `enablecors` in the
  `accepts_a_known_bool_field_and_normalizes_to_1_or_0` unit test in
  `crates/lanrurugi-api/src/settings.rs` (settings.rs:709-719)
- [X] T009 [P] Add `disable_update_check: bool` field to `AppState` in
  `crates/lanrurugi-api/src/state.rs` (state.rs:32-38); remove `enable_pass` from `AuthConfig` in
  the same file
- [X] T010 [P] Add `--disable-update-check` / `LANRURUGI_DISABLE_UPDATE_CHECK` CLI flag to
  `ServeArgs` in `crates/lanrurugi-server/src/main.rs` (main.rs:84-95), following the exact
  `no_pass`/`force_secure_cookies` `#[arg(long, env = "...", default_value_t = false)]` pattern;
  remove the `--no-pass`/`LANRURUGI_NO_PASS` flag; wire both into `AppState` construction
  (main.rs:381-388)

**Checkpoint**: Foundation ready — `Category`/`SearchParams`/`AuthMethod`/Casbin/`AppState`
primitives all exist; user story implementation can now begin.

---

## Phase 3: User Story 1 - Administrator enforces mandatory password login (Priority: P1) 🎯 MVP

**Goal**: Password-based login can no longer be disabled; every administrative/management
endpoint rejects unauthenticated callers unconditionally (spec FR-001, FR-002).

**Independent Test**: Deploy the instance, confirm no setting disables the password requirement,
and confirm every management endpoint (settings, plugins, backup, tokens) is rejected without a
valid administrator session — per spec's own Independent Test for this story and
quickstart.md §1-2.

### Tests for User Story 1 ⚠️

- [X] T011 [P] [US1] Rewrite `test_app(enable_pass: bool)` helper in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (line 32) to drop the now-removed
  `enable_pass` parameter (password enforcement is no longer configurable)
- [X] T012 [US1] Delete `nofunmode_forces_auth_even_when_enable_pass_is_off` test in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (lines 149-168) — tests a removed concept
  (research.md §8); depends on T011 (same file)
- [X] T013 [P] [US1] Remove the `"nofun_mode"` field assertion from
  `get_info_matches_recorded_serverinfo_shape` in
  `crates/lanrurugi-server/tests/contract_api.rs` (lines 225-236); keep the `"has_password"`/
  `"debug_mode"` assertions (research.md §8)
- [X] T014 [US1] Add a new integration test asserting an unauthenticated caller with no valid
  guest-mode eligibility gets `401`/redirect-equivalent from `GET /api/settings` and
  `POST /api/database/drop` in `crates/lanrurugi-server/tests/auth_flow.rs` (extends the existing
  `session_only_route_rejects_a_real_admin_token_but_accepts_a_real_session` pattern; quickstart.md
  §1)

### Implementation for User Story 1

- [X] T015 [US1] Remove the `!cfg.enable_pass && !cfg.no_fun_mode` short-circuit branch from
  `require_api_key` in `crates/lanrurugi-api/src/procedure.rs` (procedure.rs:70-77) — every request
  now proceeds to full credential evaluation; depends on T009 (`AuthConfig` no longer has
  `enable_pass`)
- [X] T016 [US1] Remove `enable_pass`/`no_fun_mode` from `LiveAuthConfig` and its `load()` in
  `crates/lanrurugi-api/src/auth.rs`; add `guest_mode_enabled` field read from the new `guestmode`
  `LRR_CONFIG` field (depends on T007)
- [X] T017 [US1] Update `login.rs::status` — drop the `!auth.enable_pass ||` half of the
  `logged_in` expression (login.rs:259-280); depends on T016
- [X] T018 [US1] Update `misc.rs::server_info` — hardcode `has_password: true`; remove `nofun_mode`
  from the response; read `debug_mode` from `state.disable_update_check` instead of
  `flag("devmode", "0")` (misc.rs:82-89); depends on T009, T010
- [X] T019 [P] [US1] Remove `enablepass`/`nofunmode` `CheckboxRow`s from
  `apps/frontend/src/pages/Settings/SecuritySection.tsx`; the password-change/token-lifetime
  fields previously nested inside `{enablepass && (...)}` become unconditionally visible
- [X] T020 [P] [US1] Remove `devmode` `CheckboxRow` from
  `apps/frontend/src/pages/Settings/GlobalSection.tsx`
- [X] T021 [US1] Drop `enablepass`/`nofunmode`/`devmode` from `useState`/`isDirty`/`handleSave` in
  `apps/frontend/src/pages/Settings/SettingsPage.tsx`; depends on T019, T020
- [X] T022 [P] [US1] Remove `enablepass`/`nofunmode`/`devmode` from `Settings` interface, remove
  `nofun_mode` from `ServerInfo` interface (`has_password`/`debug_mode` stay) in
  `apps/frontend/src/api/types.ts`
- [X] T023 [P] [US1] Remove `enablepass`/`nofunmode`/`devmode` entries from
  `SETTINGS_FIELD_SECTIONS` in `apps/frontend/src/pages/Activity/activityTarget.ts`
- [X] T024 [P] [US1] Remove `enablePassword`/`nofunMode`/`debugMode` i18n keys from all 13
  `apps/frontend/src/i18n/locales/*.json` files

**Checkpoint**: Password login can no longer be disabled; Settings page shows no
password/No-Fun/debug-mode toggles; every management endpoint rejects unauthenticated access
unconditionally. Independently deployable/demoable — satisfies SC-003 on its own, without US2/US3.

---

## Phase 4: User Story 2 - Administrator opens specific categories to unauthenticated visitors (Priority: P1)

**Goal**: A site-wide guest-mode switch plus per-category `visible_to_guest` marking together
route an eligible unauthenticated visitor into scoped browsing instead of a login redirect (spec
FR-003–FR-007, FR-013).

**Independent Test**: With guest mode enabled and at least one category marked guest-visible, open
the site in a fresh unauthenticated session and confirm the library is scoped to guest-visible
categories with a visible login entry point — per spec's own Independent Test and
quickstart.md §3-5.

### Tests for User Story 2 ⚠️

- [X] T025 [P] [US2] Add a `guestmode`/category-visibility test matrix to
  `crates/lanrurugi-server/tests/settings_toggles.rs`: guest mode off (redirect-equivalent), guest
  mode on with zero guest-visible categories (redirect-equivalent, spec US2 Scenario 3), guest mode
  on with one guest-visible category (scoped access granted) — depends on T011
- [X] T026 [P] [US2] Add a new integration test verifying an unauthenticated `guest_visitor`
  request still gets rejected from a session-only route (e.g. `POST /database/drop`) even with
  guest mode on and a category visible, in `crates/lanrurugi-server/tests/auth_flow.rs`
  (research.md §8; extends T014)
- [ ] T027 [P] [US2] ~~Add a `CategoryRepository::visible_to_guest` round-trip unit test (absent in
  Hash → `false`)~~ — SKIPPED per explicit user instruction (2026-08-27): no permanent
  "missing-field defaults" runtime fallback is written for this feature; a predating-instance's
  data is handled by a one-time, uncommitted migration tool instead (see research.md's note on this
  decision), so there is no absent-field behavior left to test. `CategoryRepository::get` now
  returns `RepositoryError::MissingField` if `visible_to_guest` is absent, matching that decision.
- [X] T028 [P] [US2] Add an `authz::subject_role` unit test for the new `guest_visitor` branch in
  `crates/lanrurugi-api/src/authz.rs`'s `mod tests` (authz.rs:231+)

### Implementation for User Story 2

- [X] T029 [US2] In `procedure.rs::require_api_key`, before the final `401` (procedure.rs:130), add
  the guest-mode-eligibility branch: when no valid `Session`/`Token` credential is present AND
  `cfg.guest_mode_enabled` AND at least one category has `visible_to_guest: true`, construct
  `AuthContext { method: AuthMethod::GuestVisitor, .. }` and proceed to `authorize_route` instead of
  rejecting; depends on T004, T015, T016
- [X] T030 [US2] Add `"visible_to_guest": 0|1` to `category_json()` in
  `crates/lanrurugi-api/src/categories.rs` (categories.rs:22-30); add `visible_to_guest: bool`
  (`#[serde(default)]`) to `CreateCategoryParams`/`UpdateCategoryParams` (categories.rs:62-67,
  169-174) and wire it into `create_category`/`update_category`; depends on T001, T002
- [X] T031 [P] [US2] Add `guest_mode_enabled: boolean` to `LoginStatus` interface in
  `apps/frontend/src/api/types.ts` (types.ts:607-612); add `guestmode: boolean` to `Settings`
  interface; add `visible_to_guest: number` to `CategoryMetadata` interface
- [X] T032 [US2] Add `guestmode` `CheckboxRow` to
  `apps/frontend/src/pages/Settings/GlobalSection.tsx`; wire into
  `apps/frontend/src/pages/Settings/SettingsPage.tsx`'s `useState`/`isDirty`/`handleSave`; depends
  on T021, T031
- [X] T033 [US2] Add a "visible to guest" checkbox next to the existing `pinned` checkbox in
  `apps/frontend/src/pages/Categories.tsx` (Categories.tsx:249-265, same UI pattern); add the field
  to `saveDetails()`'s payload (Categories.tsx:99-118); depends on T030, T031
- [X] T034 [US2] Add a new `AllowGuest` guard component in `apps/frontend/src/RouteGuards.tsx`
  (alongside existing `RequireAuth`/`RequireGuest`, RouteGuards.tsx:41-54) that renders children
  when `loginStatus.data.logged_in` OR `loginStatus.data.guest_mode_enabled`, and redirects to
  `/login` otherwise; depends on T031
- [X] T035 [US2] In `apps/frontend/src/App.tsx` (App.tsx:57-74), move the Library (`/`) and Reader
  (`/reader/:archiveId`) routes from the `RequireAuth`-wrapped group into a new
  `AllowGuest`-wrapped group; every other route under `Layout` keeps strict `RequireAuth`
  unchanged; depends on T034
- [X] T036 [US2] In `apps/frontend/src/Layout.tsx` (Layout.tsx:32-45), make the nav-link logic
  three-state: administrator (existing 8 management links), guest
  (`!logged_in && guest_mode_enabled` — a discoverable login entry point, no management links), or
  neither (existing single "Admin Login" link); depends on T031

**Checkpoint**: An administrator can enable guest mode and mark a category guest-visible; an
unauthenticated visitor lands on a scoped library view instead of `/login`, with a working login
entry point. Independently deployable/demoable on top of US1 — satisfies SC-001/SC-004/SC-005.

---

## Phase 5: User Story 3 - Guest visitor browses, reads, and searches within their authorized scope (Priority: P2)

**Goal**: A guest visitor's browsing, reading, and search/tag-filter capabilities are enforced to
stay within guest-visible categories, with zero information leakage about out-of-scope content,
and zero access to bookmark/progress/download/administrative capabilities (spec FR-008–FR-012,
FR-015, FR-016).

**Independent Test**: As an unauthenticated guest scoped to known categories, verify browsing/
reading/search stay within scope and an out-of-scope direct-access attempt fails identically to a
nonexistent archive — per spec's own Independent Test and quickstart.md §6-10.

### Tests for User Story 3 ⚠️

- [X] T037 [P] [US3] Add a `SearchParams.restrict_to_archive_ids` filtering unit test (empty set
  excludes everything, `None` is a no-op, non-empty set narrows correctly alongside other active
  filters) to `crates/lanrurugi-search/src/engine.rs`'s `mod tests` (engine.rs:715+); depends on
  T003
- [X] T038 [P] [US3] Add an integration test: guest search/browse results and tag-filter results
  never include an out-of-scope archive even when it shares a tag with an in-scope one, in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (extends T025; quickstart.md §6)
- [X] T039 [P] [US3] Add an integration test: a guest's direct request for an out-of-scope archive
  (`GET /archives/{id}/metadata`) returns the same `404` shape as a nonexistent archive ID, in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (quickstart.md §7; research.md §6)
- [X] T040 [P] [US3] Add an integration test: guest `POST` bookmark, `PUT` progress, and `GET`
  download all return non-2xx for an in-scope archive the guest can otherwise read/view, in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (quickstart.md §8)
- [X] T041 [P] [US3] Add an integration test: guest requests to `/api/plugins`, `/api/activity`,
  `/api/stats` all reject regardless of guest mode state, in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (quickstart.md §9)
- [X] T042 [P] [US3] Add an integration test: a config change (guest mode off, or last
  guest-visible category unmarked) takes effect on the guest's very next request within the same
  browsing session — no stale snapshot persists, in
  `crates/lanrurugi-server/tests/settings_toggles.rs` (spec FR-015, quickstart.md §10)

### Implementation for User Story 3

- [X] T043 [US3] In `search.rs::search_archives`/`search_archive_ids` (search.rs:184-217), when the
  caller's `AuthContext` is `AuthMethod::GuestVisitor`, compute the union of archive IDs across all
  `visible_to_guest: true` categories (via `CategoryRepository::list_all()`) and pass it as
  `SearchParams.restrict_to_archive_ids`; depends on T003, T029, T030. Also applied to
  `search_random` (not in this task's original wording, but left unguarded it was a live
  information-leakage gap of the exact kind FR-012 exists to prevent — see that handler's own
  comment).
- [X] T044 [US3] In `archives.rs::get_archive_metadata`/`get_archive_deprecated`
  (archives.rs:312-329), when the caller is `AuthMethod::GuestVisitor`, verify the archive belongs
  to at least one `visible_to_guest: true` category before proceeding; return the same
  `not_found()` response used for a genuinely nonexistent archive when it doesn't (research.md §6);
  depends on T029, T030. **Deviates from research.md's own wording**: uses
  `search::guest_visible_archive_ids` (the same static+dynamic union T043 computes), not
  `CategoryRepository::for_archive` alone — `for_archive` only sees *static* category membership,
  so an archive belonging exclusively to a *dynamic* `visible_to_guest` category would otherwise
  surface in guest search results (T043) but 404 the moment it's actually opened here, a real
  inconsistency between the two code paths caught and fixed during implementation (2026-08-27,
  confirmed with user before deviating from the research doc).
- [X] T045 [US3] Apply the same guest-scope membership check as T044 to `get_page`
  (archives.rs:1973-2057), `get_files`, and `get_page_dimensions` in
  `crates/lanrurugi-api/src/archives.rs`; depends on T044
- [X] T046 [US3] Confirm (via T040) that `/api/archives/{id}/download` (archives.rs:767-818)
  correctly denies `guest_visitor` through the Casbin policy deny rule from T006 alone — no
  application-code change needed here, this task is verification-only per plan.md's own note that
  download gating is policy-level, not code-level. Confirmed: `route_policy.csv:91` (`p,
  guest_visitor, /api/archives/:id/download, GET, deny`) already present from T006's own work; T040
  provides the live integration-test confirmation.
- [X] T047 [P] [US3] In `apps/frontend/src/pages/Library/` and `apps/frontend/src/pages/Reader/`,
  hide bookmark/progress-save/download affordances when `useLoginStatus().data.logged_in` is
  `false` (both already consume `useLoginStatus()` elsewhere in the app per plan.md). Found and
  fixed two real gaps: `ArchiveContextMenu`'s "mark as read/unread" and "download" items had no
  `loggedIn` gate at all (Reader.tsx's own bookmark icon already did); `RecentlyAddedCarousel`'s
  "Bookmarked" carousel mode tab was also ungated (confirmed with user before extending scope —
  its own `GET /bookmarks` call is technically guest-reachable, but showing a personal-bookmarks
  entry point to a guest who can never have any is a dead end, not a real feature).
- [X] T048 [P] [US3] Add a Playwright end-to-end test covering the guest journey — land on the
  site unauthenticated with guest mode on and one category visible, browse to an in-scope archive,
  open it in the reader, confirm no bookmark/download affordance is shown — in
  `apps/frontend/tests/e2e/guest-access.spec.ts` (matching the existing `tests/e2e/*.spec.ts`
  convention; extends 003-ui-test-automation's Playwright layer). Written and lint/type-checked
  clean; NOT actually run end-to-end (2026-08-27) — this sandbox's tooling is split across two
  incompatible environments (a Rust-toolchain build container with no Redis/Playwright reachable
  from the host, and the running `lrr-dev` container, which has Redis/pnpm/Playwright but no Rust
  toolchain to build `target/debug/lanrurugi-server` the fixture expects), so `fixtures.ts`'s own
  per-worker `spawn("redis-server", ...)` step never got a real binary to launch against. Needs a
  real run (`pnpm exec playwright test guest-access.spec.ts`) in an environment with both before
  this is considered fully verified — flagged, not silently assumed passing.
- [X] T049 [P] [US3] Add a Vitest unit test for `RouteGuards.tsx`'s new `AllowGuest` branch
  (renders children when logged in OR guest-mode-eligible, redirects otherwise) in
  `apps/frontend/tests/unit/routeGuards.test.tsx` (matching the existing `tests/unit/*.test.ts(x)`
  convention); depends on T034. 4 tests, all passing (optimistic-render-while-loading, real
  session, eligible guest, neither-so-redirect).

**Checkpoint**: All three user stories independently functional — guest browsing/reading/search
stays strictly within scope, zero information leakage, zero write/download/admin capability.
Satisfies SC-002 in full.

---

## Phase 6: Polish & Cross-Cutting Concerns

**Purpose**: Cleanup and validation that spans all three stories.

- [X] T050 [P] Remove the entire `anonymous` Casbin subject block from
  `crates/lanrurugi-api/policy/route_policy.csv` if not already fully removed as part of T006 —
  final verification pass (no code path should produce a request with no `AuthContext` and full
  access after this feature). Confirmed: zero `p, anonymous, ...` lines remain, only a historical
  mention in a comment.
- [X] T051 [P] Add a short explanatory comment block above the new `guest_visitor` policy section
  in `crates/lanrurugi-api/policy/route_policy.csv`, matching the existing inline-comment
  convention for `anonymous`/`token_guest` (research.md §3 rationale — why the download deny is
  guest_visitor-specific). Already present (route_policy.csv:10-19, 66-67, 85-90).
- [X] T052 Run `cargo test --workspace` (via `mise run test`, per this repo's own resource-capped
  script convention) and confirm all Phase 2-5 tests pass, plus no pre-existing test broke. Run
  2026-08-27 (detached from the agent's own process tree per this repo's own CLAUDE.md rule). Every
  007-specific test passed (`guest_visitor_is_read_only_and_cannot_download_raw_archives`,
  `restrict_to_archive_ids_narrows_the_candidate_set`,
  `guest_visitor_reaches_ordinary_routes_but_not_session_only_ones`,
  `unauthenticated_request_is_rejected_when_guest_mode_is_off`,
  `guest_mode_and_category_visibility_matrix`,
  `guest_search_excludes_out_of_scope_archive_sharing_a_tag`,
  `guest_metadata_request_for_out_of_scope_archive_404s_like_nonexistent`,
  `guest_cannot_bookmark_save_progress_or_download_an_in_scope_archive`,
  `guest_cannot_reach_plugins_activity_or_stats_regardless_of_guest_mode`,
  `guest_eligibility_change_takes_effect_on_the_very_next_request`). Two pre-existing, unrelated
  failures (`lanrurugi-storage::bootstrap::tests::succeeds_against_a_real_dir_and_reachable_redis`,
  `lanrurugi-api::plugins::tests::ehdl_plugin_produces_a_real_downloadable_url_with_a_correctly_decoded_filename`)
  — both fail with "Connection refused (os error 111)"/an external-network dependency, confirmed
  unrelated to this feature's changes, same as recorded in an earlier run this session.
- [ ] T053 Run the full quickstart.md validation (all 11 scenarios) against a real running instance.
  NOT run (2026-08-27) — the user explicitly deferred `mise run dev-rebuild` for this session
  ("继续实装 Phase 4/5，暂不 rebuild"). Most scenarios (§3-10) have an equivalent, already-passing
  automated test (`settings_toggles.rs`/`auth_flow.rs`, see T052's own list); §1, §2, §11 do not
  and still need a real running instance. Still needs a real rebuild + manual/curl pass through all
  11 before this feature is considered release-ready.
- [x] T054 [P] Update `README.md` (`en`/`ja`/`zh`) "Improvements over LANraragi" section to mention
  restricted guest access mode, per this repo's own "before pushing, check whether README needs
  updating" convention — only if this feature is being pushed/merged as user-facing-complete
  Done (2026-08-27): all three README files' status paragraph, "Improvements" list, and
  "Documentation" section updated to mention 007 (noting T053's own still-pending live-instance
  validation). Also added issue #97's stamp/bookmark linking as its own bullet in the same pass
  since it landed in the same commit and had no README mention yet either.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: Empty — no dependencies, nothing to wait on.
- **Phase 2 (Foundational)**: No dependencies on Phase 1. **BLOCKS all user stories** — T001-T010
  must all complete before any US1/US2/US3 task starts.
- **Phase 3 (US1)**: Depends on Phase 2 (specifically T004, T005, T006, T007, T009, T010). No
  dependency on US2/US3.
- **Phase 4 (US2)**: Depends on Phase 2 (T001, T002, T004-T007). Depends on Phase 3's T015/T016
  (the `require_api_key`/`LiveAuthConfig` rewrite US2's T029 extends) — **US2 cannot start until
  US1's backend tasks (T015-T018) are done**, even though US2 is also P1. Frontend US2 tasks
  (T031-T036) additionally depend on US1's frontend `SettingsPage.tsx` rewrite (T021).
- **Phase 5 (US3)**: Depends on Phase 2 (T003) and Phase 4 (T029, T030 — needs `AuthMethod::
  GuestVisitor` actually reachable and `visible_to_guest` actually settable before scope-filtering
  has anything real to filter against).
- **Phase 6 (Polish)**: Depends on all of Phases 3-5 being complete.

### User Story Dependencies

Unlike a typical spec-kit feature where P1 stories are independent of each other, **US1 and US2
have a real backend dependency** here: US2's guest-eligibility branch (T029) is inserted directly
into the same `require_api_key` function US1's T015 rewrites, and reads the same `LiveAuthConfig`
US1's T016 changes. This is a correctness dependency, not an artificial one — the two toggles
being replaced (`enable_pass`/`no_fun_mode`) and the one being added (`guestmode`) all live in the
same auth-decision function. **US2 backend work should start only after T015/T016/T017/T018 land**,
even though both stories are P1.

US3 is cleanly dependent on US2 (it scopes content *within* the access US2 grants) and was already
ranked P2 in spec.md for exactly this reason.

### Within Each User Story

- Tests written first (marked ⚠️), verified to fail before implementation.
- Backend model/auth-context changes before backend endpoint changes.
- Backend endpoint changes before frontend consumption of them.
- Story checkpoint reached only once both backend and frontend tasks for that story are done.

### Parallel Opportunities

- All Phase 2 tasks marked [P] (T002, T003, T009, T010) can run in parallel with each other, but
  not with T001 (T002 depends on T001's struct field existing) or T006/T007 (independent, so those
  four ARE also parallelizable with T002/T003/T009/T010 — the only real serialization in Phase 2 is
  T001→T002).
- Within US1: T011-T014 (tests) can all run in parallel with each other (different files/
  independent assertions) except T012 depends on T011 (same file, sequential edit). T019-T024
  (frontend) are all [P] against each other and against the backend tasks T015-T018 (different
  files, though logically the frontend removals should follow the backend field removals to avoid
  a broken intermediate state if deployed mid-story).
- Within US2: T025-T028 (tests) are all [P]. T031 unblocks T032/T033/T034 which can then proceed in
  parallel with each other.
- Within US3: T037-T042 (tests) are all [P]. T044→T045 is sequential (same guard pattern, T045
  extends what T044 establishes). T047-T049 (frontend) are all [P] against each other and against
  the backend tasks T043-T046.

---

## Parallel Example: Phase 2 (Foundational)

```bash
# After T001 completes, launch these together:
Task: "Read/write visible_to_guest in CategoryRepository::get/save (repository.rs)"
Task: "Add restrict_to_archive_ids to SearchParams (engine.rs)"
Task: "Add disable_update_check to AppState (state.rs)"
Task: "Add --disable-update-check CLI flag (main.rs)"

# Independently, these four can also run in parallel with the above:
Task: "Add AuthMethod::GuestVisitor variant (auth_context.rs)"
Task: "Add guest_visitor match arm to subject_role (authz.rs)"
Task: "Add guest_visitor Casbin policy block + remove anonymous block (route_policy.csv)"
Task: "Add guestmode to BOOL_FIELDS, remove enablepass/nofunmode/devmode (settings.rs)"
```

## Parallel Example: User Story 1 tests

```bash
Task: "Rewrite test_app(enable_pass: bool) helper (settings_toggles.rs)"
Task: "Remove nofun_mode assertion from get_info_matches_recorded_serverinfo_shape (contract_api.rs)"
Task: "Add unauthenticated-management-endpoint-rejected test (auth_flow.rs)"
```

---

## Implementation Strategy

### MVP First (User Story 1 Only)

1. Complete Phase 2: Foundational (CRITICAL — blocks everything).
2. Complete Phase 3: User Story 1.
3. **STOP and VALIDATE**: run quickstart.md §1-2, confirm password login cannot be disabled and
   every management endpoint rejects unauthenticated access.
4. This alone is a real, deployable security hardening — SC-003 is satisfied without US2/US3 ever
   landing, so US1 is a legitimate standalone MVP even though the feature's actual value
   proposition (guest browsing) isn't live yet.

### Incremental Delivery

1. Foundational → US1 (mandatory password, MVP) → validate independently → deploy/demo.
2. Add US2 (guest-mode opt-in browsing) → validate independently (quickstart.md §3-5) →
   deploy/demo — guests can now browse, but scope-narrowing (US3) isn't enforced on search/
   direct-access yet, so **do not enable guest mode in production between US2 and US3 landing** —
   see Edge Cases in spec.md; US2 alone technically routes guests into the library view, but only
   US3 enforces the category-scope boundary on search/direct-archive-access, so shipping US2
   without US3 would let a routed-in guest reach out-of-scope content. Treat US2+US3 as a combined
   deployment unit despite being separate implementation phases.
3. Add US3 (scope enforcement + capability boundary) → validate independently (quickstart.md
   §6-10) → this is the point at which guest mode is actually safe to enable in production.
4. Phase 6 polish/README once all three stories are live.

### Parallel Team Strategy

With multiple developers:

1. Team completes Phase 2 (Foundational) together — it's small and every story needs all of it.
2. Once Foundational is done:
   - Developer A: US1 backend (T015-T018) — unblocks US2 backend.
   - Developer B: US1 frontend (T019-T024) — independent of A, can start immediately.
3. Once US1 backend lands:
   - Developer A: US2 backend (T029-T030) → then US3 backend (T043-T046).
   - Developer B: US2 frontend (T031-T036) → then US3 frontend (T047-T049).
4. Tests (T011-T014, T025-T028, T037-T042) can be written by either developer ahead of the
   implementation task they cover, per the "write tests first" convention.

---

## Notes

- **US1/US2 backend dependency is real, not a template artifact** — see Dependencies & Execution
  Order above. Do not attempt to parallelize US1 and US2 backend work; the frontend halves are
  genuinely independent.
- **Do not enable guest mode in production until US3 lands** — US2 alone routes guests past the
  login redirect but does not yet enforce category-scope boundaries on search/direct-archive
  access; per spec Edge Cases, an administrator turning guest mode on before US3 ships would be a
  real information-leakage exposure, not merely an incomplete feature.
- [P] tasks = different files, no dependencies within their listed constraints above.
- [Story] label maps task to specific user story for traceability back to spec.md's FR-###/SC-###.
- Verify each new test actually fails against pre-implementation code before implementing (tests
  are listed before implementation tasks within each story for this reason).
- Commit after each task or logical group.
- Every Rust file-editing task references the same file/line anchors already verified against real
  code in research.md/plan.md — no task here introduces a file path not already confirmed to exist.
