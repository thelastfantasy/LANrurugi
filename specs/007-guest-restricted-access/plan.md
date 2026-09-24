# Implementation Plan: Restricted Guest Access Mode

**Branch**: `007-guest-restricted-access` | **Date**: 2026-08-27 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/007-guest-restricted-access/spec.md`

## Summary

Replace the legacy `enablepass`/`nofunmode` combination (which only ever produced "fully open" or
"fully locked" states) with a strict, non-configurable password requirement for every
administrative function, paired with a new opt-in **restricted guest access** mode: a
site-wide "guest mode" switch plus per-`Category` `visible_to_guest` marking together determine
whether an unauthenticated visitor is routed into a scoped, read-only browsing experience (list,
read, search/tag-filter — all confined to guest-visible categories) instead of being redirected to
`/login`. The new `guest_visitor` Casbin subject is governed entirely through
`route_policy.csv` (route-level allow/deny), while category-level content scoping — which Casbin's
existing RBAC model has no mechanism for — is enforced in application code via a new
`SearchParams.restrict_to_archive_ids` field that reuses the search engine's existing
retain-a-candidate-set filtering pattern. `devmode` (confirmed to have zero server-side behavior
already) is removed from the Settings page entirely and replaced by a deploy-time CLI flag/env var,
following the exact pattern `--no-pass`/`LANRURUGI_NO_PASS` already establishes.

## Technical Context

**Language/Version**: Rust (stable channel, pinned via `mise`, matching 001's existing pin) for
the auth middleware, Casbin policy, Category model, and search-engine changes; TypeScript (React
19) for the Settings-page removals, Category management UI addition, route-guard restructuring,
and the new guest-mode-aware Library/Reader views.

**Primary Dependencies**: No new dependency. Reuses the existing `casbin` crate (already a
workspace dependency, `crates/lanrurugi-api/src/authz.rs`), the existing `deadpool_redis`-backed
`CategoryRepository` (`crates/lanrurugi-storage/src/repository.rs`), the existing `clap`
`#[arg(long, env = "...")]` CLI-flag convention (`crates/lanrurugi-server/src/main.rs`), and the
existing TanStack Query + React Router stack on the frontend.

**Storage**: Redis (reused as-is, per Constitution Principle I). `Category`'s existing Hash
representation (`SET_<timestamp>` key) gains one new field, `visible_to_guest` (`"0"`/`"1"`,
identical convention to the existing `pinned` field) — additive, and absent-means-`false` for
every category that predates this feature, so no migration script is needed. A new top-level
`LRR_CONFIG` boolean field, `guestmode` (alongside the existing `enablecors`/`localprogress`-style
`BOOL_FIELDS`), stores the site-wide guest-mode switch. The `enablepass`/`nofunmode`/`devmode`
fields are removed from `BOOL_FIELDS` and, for pre-existing deployments, simply become inert
leftover Hash fields (harmless — nothing reads them after this change; no explicit Redis cleanup
required for correctness, though a one-time `HDEL` MAY be included for hygiene).

**Testing**: `cargo test` — new unit tests for `authz::subject_role`'s new `guest_visitor` branch,
`SearchParams.restrict_to_archive_ids` filtering, and `CategoryRepository`'s `visible_to_guest`
round-trip (default-false-when-absent); new integration tests in
`crates/lanrurugi-server/tests/` for the full unauthenticated-guest request flow (guest mode
on/off × categories marked/unmarked × in-scope/out-of-scope archive access × the `download`/
`bookmark`/`progress` endpoints specifically verified denied); existing
`crates/lanrurugi-server/tests/auth_flow.rs`,
`crates/lanrurugi-server/tests/settings_toggles.rs`, and
`crates/lanrurugi-server/tests/contract_api.rs` updated per the removal/rewrite list in
research.md §8. Frontend: existing Vitest/Playwright layers (003-ui-test-automation) extended with
a guest-mode Library/Reader journey and a route-guard unit test for the new three-state
(admin/guest/anonymous) branch in `RouteGuards.tsx`.

**Target Platform**: Linux server (unchanged from 001 — no new deployable, no new target).

**Project Type**: Web application (unchanged from 001 — extends the existing Rust backend + React
SPA, not a new deployable).

**Performance Goals**: SC-002 (zero information leakage — an unauthenticated guest's search/browse
results, tag suggestions, and direct-access attempts must never surface anything outside their
authorized category scope; this is a correctness bar, not a latency one). SC-001 (an administrator
can enable guest mode and mark categories in under 2 minutes using only existing category-
management UI — no new page, just one new checkbox and one new global toggle).

**Constraints**: Constitution Principle I (no destructive Redis-data change — `visible_to_guest`
is additive with a safe absent-default, exactly like `pinned`'s own precedent); Principle II (the
`/api/info` `has_password`/`nofun_mode` fields are part of the documented legacy third-party API
contract — `has_password` MUST continue to be present and boolean, now hardcoded `true` rather
than reflecting a Redis-configurable value, since password protection can no longer be disabled;
`nofun_mode` is removed from the response since the concept it described no longer exists —
existing third-party clients that merely display or ignore this field are unaffected, none are
known to gate functionality on `nofun_mode` specifically); Principle VI (this is a Phase 1
addendum — like 002/003/005/006 — independent of and non-blocking toward Phase 2's OCR/translation
work).

**Scale/Scope**: 3 user stories (P1 mandatory admin password, P1 guest-mode opt-in browsing, P2
guest capability boundary), 17 functional requirements (FR-001–FR-017), 3 key entities (Category
extension, Guest Mode Setting, Guest Visitor auth context variant). Single-owner/single-instance
deployment scope, matching 001 — no multi-tenant concerns.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | New Redis fields must be additive, no existing key/field shape destructively changed | **PASS** — `Category.visible_to_guest` follows `pinned`'s exact absent-default-false pattern (`repository.rs:310-316`); removed `enablepass`/`nofunmode`/`devmode` fields become inert (not deleted, not required) on upgrade, per FR-013's mandated migration behavior |
| II. API Contract Fidelity (Phase 1) | `/api/info`'s documented third-party fields must keep parsing correctly for existing clients | **PASS with a documented, spec-mandated exception** — `has_password` stays present/boolean (now hardcoded `true`); `nofun_mode` is removed as its underlying concept is removed (FR-014 explicitly scopes this as an accepted compatibility trade-off, not an oversight) |
| III. Resource-Conscious, Genuinely Concurrent Architecture | New guest-scoping logic must not introduce blocking/non-async work | **PASS** — `restrict_to_archive_ids` filtering reuses `search()`'s existing async `HashSet::retain` pattern (`engine.rs:107-115`); no new blocking I/O, no CPU-bound work needing `rayon` |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | Plugin sandbox model must not be weakened | **N/A** — this feature touches auth/category/search, not the plugin runtime |
| V. Secrets & Network Trust Boundaries | N/A (Phase 2-only) | **N/A** — Phase 1 scope only |
| VI. Phased Scope Discipline | Must not smuggle Phase 2 concerns in, must not block Phase 1 | **PASS** — Phase 1 addendum, independent of and non-blocking toward 004 |
| VII. Frontend Engineering Discipline & Legacy UI Fidelity | New/changed UI must follow page-file organization, real stylesheet classes for conditional layout, and (if reproducing legacy UI) computed-style verification | **PASS** — the new guest-visible checkbox in `Categories.tsx` reuses the existing `pinned` checkbox's exact UI pattern (`Categories.tsx:249-265`); the route-guard restructuring is logic, not visual, so no legacy-UI-fidelity verification applies; any new guest-mode-specific visual element (e.g. a "browsing as guest" banner) MUST go through the existing CLAUDE.md computed-style verification procedure at implementation time |

No violations requiring Complexity Tracking.

## Project Structure

### Documentation (this feature)

```text
specs/007-guest-restricted-access/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-core/
│   └── src/
│       └── entities.rs          # Category gains `visible_to_guest: bool` (entities.rs:118-135)
├── lanrurugi-storage/
│   └── src/
│       └── repository.rs        # CategoryRepository::get/save read+write the new field
│                                 # (repository.rs:310-316 read, 396-404 write), following
│                                 # `pinned`'s exact absent-default-false pattern
├── lanrurugi-search/
│   └── src/
│       └── engine.rs             # SearchParams gains `restrict_to_archive_ids:
│                                 # Option<HashSet<ArchiveId>>`; `search()` retains it via the
│                                 # same HashSet::retain pattern already used for `category`/
│                                 # `untaggedonly`/`tankonly` (engine.rs:107-134)
├── lanrurugi-api/
│   └── src/
│       ├── auth_context.rs      # AuthMethod gains a new variant (name finalized in
│       │                        # research.md — auth_context.rs:10-19)
│       ├── auth.rs              # LiveAuthConfig loses enable_pass/no_fun_mode, gains
│       │                        # guest_mode_enabled; load() reads new `guestmode` field
│       ├── procedure.rs         # require_api_key: remove the enable_pass&&no_fun_mode
│       │                        # short-circuit (procedure.rs:70-77); add the
│       │                        # unauthenticated-but-guest-mode-eligible branch before the
│       │                        # final 401 (procedure.rs:130)
│       ├── authz.rs             # subject_role gains a `guest_visitor` match arm
│       │                        # (authz.rs:49-62); route/activity Enforcer loading unchanged
│       ├── login.rs             # status(): logged_in drops the `!auth.enable_pass ||` half;
│       │                        # response gains `guest_mode_enabled` (login.rs:259-280)
│       ├── categories.rs        # category_json() gains "visible_to_guest": 0|1
│       │                        # (categories.rs:22-30); CreateCategoryParams/
│       │                        # UpdateCategoryParams gain `visible_to_guest: bool`
│       │                        # (categories.rs:62-67, 169-174)
│       ├── search.rs            # search_archives/search_archive_ids: when the caller is
│       │                        # guest_visitor, build `restrict_to_archive_ids` from the
│       │                        # union of guest-visible categories' archives before calling
│       │                        # `search()` (search.rs:128-217)
│       ├── archives.rs          # get_archive_metadata/get_page/get_files/
│       │                        # get_page_dimensions: guest_visitor requests verify archive
│       │                        # membership in a guest-visible category before proceeding
│       │                        # (archives.rs:312-329 metadata, 1973-2057 page); download
│       │                        # endpoint (archives.rs:767-818) gets an explicit policy deny,
│       │                        # not application-code gating
│       ├── misc.rs              # server_info(): has_password hardcoded true; nofun_mode
│       │                        # field removed; debug_mode reads AppState's new deploy-time
│       │                        # field instead of the removed `devmode` LRR_CONFIG entry
│       │                        # (misc.rs:82-89)
│       ├── settings.rs          # BOOL_FIELDS loses enablepass/nofunmode/devmode, gains
│       │                        # guestmode (settings.rs:224-236); TOKEN_AUTH_FORBIDDEN_
│       │                        # SETTINGS_FIELDS loses enablepass/nofunmode (settings.rs:
│       │                        # 365-370); unit test at settings.rs:709-719 swaps its
│       │                        # example field from `devmode` to a still-existing one
│       │                        # (e.g. `enablecors`)
│       └── state.rs             # AuthConfig loses enable_pass (now implicit/always-true);
│                                 # AppState gains a new top-level `disable_update_check: bool`
│                                 # field (state.rs:32-38)
├── lanrurugi-server/
│   ├── src/
│   │   └── main.rs               # ServeArgs: --no-pass removed; new
│   │                             # --disable-update-check / LANRURUGI_DISABLE_UPDATE_CHECK
│   │                             # flag added following the exact `no_pass`/
│   │                             # `force_secure_cookies` pattern (main.rs:84-95, 381-388)
│   └── tests/
│       ├── auth_flow.rs          # session_only_route_rejects_... unaffected; new test added
│       │                         # for guest_visitor hitting a session-only route (deny)
│       ├── settings_toggles.rs   # nofunmode_forces_auth_even_when_enable_pass_is_off DELETED;
│       │                         # test_app(enable_pass: bool) helper simplified (no longer
│       │                         # parameterized on a since-removed concept); new tests added
│       │                         # for guest-mode-on/off × category-marked/unmarked matrix
│       └── contract_api.rs       # get_info_matches_recorded_serverinfo_shape: drop the
│                                 # "nofun_mode" field assertion (lines 225-236)
└── lanrurugi-api/
    └── policy/
        └── route_policy.csv      # `anonymous` role's entire block removed; new `guest_
                                 # visitor` block added following `token_guest`'s exact
                                 # structure (GET-only allow + the same 15-entry deny list)
                                 # PLUS one guest_visitor-specific deny not present in any
                                 # existing role: `/api/archives/:id/download` (GET) — see
                                 # research.md §4 for why this is the one GET endpoint that
                                 # must NOT inherit the default GET-allow

apps/frontend/
└── src/
    ├── api/
    │   └── types.ts               # LoginStatus gains `guest_mode_enabled: boolean`
    │                              # (types.ts:607-612); Settings loses enablepass/nofunmode/
    │                              # devmode, gains guestmode; CategoryMetadata gains
    │                              # `visible_to_guest: number` (0|1, matching `pinned`'s own
    │                              # existing `number` convention, not `boolean`)
    ├── RouteGuards.tsx            # RequireAuth's redirect branch gains a guest_mode_enabled
    │                              # check; new `AllowGuest` guard component wraps only
    │                              # Library (`/`) and Reader (`/reader/:archiveId`) — every
    │                              # other route under Layout keeps strict RequireAuth
    │                              # unchanged (RouteGuards.tsx:41-54)
    ├── App.tsx                    # Route tree split: Library/Reader move from the
    │                              # RequireAuth-wrapped group to the new AllowGuest-wrapped
    │                              # group (App.tsx:57-74)
    ├── Layout.tsx                 # Nav link logic becomes three-state (admin/guest/neither)
    │                              # instead of two-state (Layout.tsx:32-45)
    ├── pages/
    │   ├── Categories.tsx         # New "visible to guest" checkbox next to the existing
    │   │                          # `pinned` checkbox, same UI pattern (Categories.tsx:
    │   │                          # 249-265); saveDetails() payload gains the new field
    │   │                          # (Categories.tsx:99-118)
    │   ├── Settings/
    │   │   ├── SecuritySection.tsx   # enablepass/nofunmode CheckboxRows removed; the
    │   │   │                         # password-change/token-lifetime fields previously
    │   │   │                         # nested inside `{enablepass && (...)}` become
    │   │   │                         # unconditionally visible
    │   │   ├── GlobalSection.tsx     # devmode CheckboxRow removed; new guestmode
    │   │   │                         # CheckboxRow added
    │   │   └── SettingsPage.tsx      # useState/isDirty/handleSave: drop enablepass/
    │   │                             # nofunmode/devmode, add guestmode
    │   ├── Library/                  # and Reader/ — gain guest-mode-aware UI: bookmark/
    │   │                             # progress/download affordances hidden when
    │   │                             # loginStatus.data.logged_in is false (both already
    │   │                             # consume useLoginStatus() elsewhere in the app)
    │   └── Activity/
    │       └── activityTarget.ts     # SETTINGS_FIELD_SECTIONS loses enablepass/nofunmode/
    │                                 # devmode entries, gains guestmode
    └── i18n/
        └── locales/*.json            # enablePassword/nofunMode/debugMode keys removed
                                       # across all 13 language files; new guestMode-related
                                       # keys added
```

**Structure Decision**: Extends the existing Phase 1 Cargo workspace/frontend app — no new crate,
no new deployable. The one new cross-cutting concept (`restrict_to_archive_ids`) lives in
`lanrurugi-search` (where the existing single-category filter already lives) rather than as a
separate guest-specific module, keeping all "narrow the candidate archive-id set" logic in one
place. Auth-context/Casbin changes stay within `lanrurugi-api`, matching the existing
`auth_context.rs`/`authz.rs`/`procedure.rs` three-file split.

## Complexity Tracking

*No Constitution Check violations — table omitted.*
