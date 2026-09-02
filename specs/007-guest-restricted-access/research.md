# Research: Restricted Guest Access Mode

## 1. New `AuthMethod` variant naming

**Decision**: `AuthMethod::GuestVisitor` (no payload — unlike `Token { id, role }`, a guest
visitor carries no persistent identity per spec Assumptions: "every unauthenticated visit is
evaluated fresh against current guest-mode/category configuration").

**Rationale**: `AuthMethod`'s existing two variants are `Session` (real admin) and `Token { id,
role }` (API token, admin or guest role). A guest visitor is neither — it's an unauthenticated
request that gets *elevated* to a scoped identity purely because guest mode + category config
allow it, not because any credential was presented. A payload-less variant named after the Casbin
subject it maps to (`guest_visitor`, see §2) keeps `auth_context.rs`'s three variants readable as
"how was this request authenticated" without conflating it with `TokenRole::Guest` (API-token
guest — method-level read-only, no category scoping, entirely different mechanism per the prior
research already on record).

**Alternatives considered**: Reusing `TokenRole::Guest` inside a new `AuthMethod::Guest(TokenRole)`
— rejected because `TokenRole` is specifically the API-token role enum
(`lanrurugi-storage::api_tokens::TokenRole`); stretching it to also mean "unauthenticated visitor"
would make `is_guest_token()` (`auth_context.rs:39-47`, which today correctly returns `false` for
non-token methods) either wrong or need special-casing, for no real benefit.

## 2. Casbin subject string

**Decision**: `"guest_visitor"` (matches the `AuthMethod::GuestVisitor` variant name, lowercased +
snake_cased — consistent with the existing four subjects' naming: `session`, `anonymous`,
`token_admin`, `token_guest`).

**Rationale**: Keeping the Rust variant name and the Casbin subject string visually paired (one is
just `snake_case` of the other) makes `authz::subject_role`'s match arm self-documenting and
matches the existing four subjects' own naming convention exactly.

## 3. Route-level policy content for `guest_visitor`

**Decision**: A new block in `route_policy.csv`, placed after the existing `token_guest` block,
structured identically:

```
p, guest_visitor, /api/*, GET, allow

# guest_visitor deny list — identical to token_guest's own deny list (see that block's own
# rationale for each entry) PLUS one guest_visitor-specific entry token_guest does not need:
# downloading a raw archive file is meaningfully different from the reader's own per-page image
# endpoint (which guest_visitor legitimately needs — spec FR-008), and token_guest's existing
# deny list was never written with an unauthenticated, content-scoped visitor in mind.
p, guest_visitor, /api/activity, DELETE, deny
p, guest_visitor, /api/activity/:id, DELETE, deny
p, guest_visitor, /api/activity/retention, PUT, deny
p, guest_visitor, /api/archives, DELETE, deny
p, guest_visitor, /api/archives/:id/download, GET, deny
p, guest_visitor, /api/database/drop, POST, deny
p, guest_visitor, /api/settings/password, POST, deny
p, guest_visitor, /api/tokens, GET, deny
p, guest_visitor, /api/tokens, POST, deny
p, guest_visitor, /api/tokens/:id, PATCH, deny
p, guest_visitor, /api/tokens/:id, DELETE, deny
p, guest_visitor, /api/plugin-wizard/lookup, POST, deny
p, guest_visitor, /api/plugin-wizard/analyze-login, POST, deny
p, guest_visitor, /api/plugin-wizard/generate/start, POST, deny
p, guest_visitor, /api/plugin-wizard/generate/stream/:id, GET, deny
p, guest_visitor, /api/plugin-wizard/trial-run, POST, deny
p, guest_visitor, /api/plugin-wizard/save, POST, deny
```

**Rationale**: `token_guest`'s existing deny list (verified in research: 15 entries covering
session-only routes + plugin-wizard) already excludes every write/management operation; reusing it
verbatim keeps `guest_visitor` from silently drifting out of sync with `token_guest`'s own
session-only-route coverage as that list evolves. The one addition (`download`, GET) is the single
case where `guest_visitor`'s "GET is generally safe" default assumption breaks — downloading the
raw archive file is a bulk-export capability spec FR-009 explicitly excludes, while `GET
/archives/:id/page` (the reader's per-page image fetch, spec FR-008 explicitly requires) must
remain allowed. Both are GET; only policy-level deny can distinguish them, since Casbin's `obj`
matching is path-based (`keyMatch2`), not semantic.

Bookmark (`POST/DELETE /archives/:id/bookmarks/:page`) and progress
(`PUT /archives/:id/progress/:page`, `PUT /tankoubons/:id/progress/:page`) endpoints need **no**
explicit deny entries — all three are non-GET, so `guest_visitor`'s GET-only allow rule already
excludes them by construction (research confirmed: `bookmarks.rs:28-46`, `archives.rs:217,1518`,
`tankoubons.rs:94-97,766` — none are GET).

**Alternatives considered**: A completely independent, hand-written deny list for `guest_visitor`
— rejected because it would duplicate `token_guest`'s 15 entries with no behavioral difference,
doubling future maintenance burden for zero benefit (any future session-only route added to one
list would need the exact same addition to the other, with no compiler/test enforcement of that
sync today — the CSV is data, not code).

## 4. Category-level content scoping mechanism

**Decision**: `SearchParams` (`lanrurugi-search::engine`) gains a new field,
`restrict_to_archive_ids: Option<HashSet<ArchiveId>>`, applied via the exact same
`HashSet::retain`-on-the-candidate-set pattern the engine already uses for `category`/
`untaggedonly`/`tankonly`/`newonly` (`engine.rs:107-134`). When a `guest_visitor` request reaches
`search_archives`/`search_archive_ids` (`search.rs:184-217`), the handler computes the union of
archive IDs across every `visible_to_guest: true` category and passes that set as
`restrict_to_archive_ids`; `search()` retains only IDs present in that set, on top of whatever
other filters (keyword, tag, the existing single-`category` param) already apply.

For single-resource endpoints (`GET /archives/:id/metadata`, `GET /archives/:id/page`, `GET
/archives/:id/files`, `GET /archives/:id/page-dimensions`), the same guest-visible-category-union
check is applied directly: the handler looks up the archive's category memberships
(`CategoryRepository::for_archive`, `repository.rs:333-345`) and denies (404, not 403 — see §6)
if none of them is guest-visible.

**Rationale**: This was researched and rejected in favor of a cleaner option than either
alternative surfaced by the initial code-level investigation:

- *Handler-level post-filter after pagination* (discard out-of-scope entries from an already-paged
  result set) was the initially identified "more realistic" option, but has a real, spec-relevant
  flaw: it desyncs `recordsFiltered`/`recordsTotal` from the actually-returned `data.len()`,
  which risks leaking *how many* archives exist outside the guest's scope (a page that should have
  20 results silently returns fewer) — a subtle instance of exactly the information leakage
  SC-002/FR-012 prohibit.
- *Extending the search engine's query layer with true OR-across-categories* was assumed to be a
  large change requiring new Redis query primitives — but reading `engine.rs` directly (not just
  its external signature) shows the entire engine already operates by narrowing a plain in-memory
  `HashSet<String>` of candidate IDs through a chain of independent `retain()` calls per filter.
  Adding one more filter of the same shape is a small, additive change, not an engine redesign.

Using `HashSet<ArchiveId>` (computed once per request from the guest-visible categories' own
`archives`/`search`-resolved membership) rather than re-deriving it via a Casbin-style rule keeps
category scoping — inherently a *resource-level*, per-request-data concern — out of Casbin
entirely, consistent with the existing architectural split documented in `authz.rs:8-20` (route
Enforcer = RBAC on paths; activity Enforcer = ABAC on individual records; neither was ever meant to
carry a third, content-set-membership concern).

**Alternatives considered**: See above — both rejected with concrete reasons, not merely listed.

## 5. `/api/info` compatibility fields (Constitution Principle II)

**Decision**: `has_password` remains present, boolean, now unconditionally `true` (never reads
Redis). `nofun_mode` is removed from the response entirely. `debug_mode` remains present but now
reflects a deploy-time flag (§7) rather than a Redis-configurable value.

**Rationale**: `has_password`/`nofun_mode`/`debug_mode` are part of legacy's own third-party
`ServerInfo` contract (`misc.rs:82-89`, verified against `~/LANraragi`'s OpenAPI schema per the
file's own docstring). Principle II requires *existing* fields to keep parsing correctly for
third-party clients (Tachiyomi/Mihon-style extensions), not that every field's *meaning* stays
frozen forever when the underlying capability is deliberately removed. `has_password` staying
`true` is not a lie to third-party clients — password protection genuinely is always on now, so a
client that conditionally shows a password prompt based on this field will simply always show it,
which is correct. `nofun_mode` describes a concept (login required even without a password wall)
that stops being meaningful once the password wall itself can't be disabled — no known third-party
client is known to gate behavior on that specific field (only on `has_password`), so removing it
is a documented, accepted narrowing rather than a silent contract violation.

**Alternatives considered**: Keeping `nofun_mode` present but hardcoded — rejected as actively
misleading (implies the concept still exists and could theoretically be `false`, which is no
longer true), versus simply omitting a field that has no meaning left to report.

## 6. Guest-visitor access-denial response shape

**Decision**: An unauthenticated guest visitor requesting an archive/category outside their scope
receives the exact same response shape (404 "doesn't exist") as a genuinely nonexistent
archive/category ID — not a 403.

**Rationale**: Directly required by spec FR-011 ("must be denied without revealing whether that
archive exists ... indistinguishable"). This matches the existing `not_found()` helper already
used throughout `categories.rs`/`archives.rs` for genuine not-found cases — no new response
helper needed, just routing the "exists but out of scope" case through the same function call as
the "doesn't exist" case.

**Alternatives considered**: 403 Forbidden — rejected outright; explicitly contradicts FR-011.

## 7. `devmode` → deploy-time configuration

**Decision**: New CLI flag `--disable-update-check` / environment variable
`LANRURUGI_DISABLE_UPDATE_CHECK`, following the exact `#[arg(long, env = "LANRURUGI_NO_PASS",
default_value_t = false)]` pattern already used for `no_pass`/`force_secure_cookies`/`no_watch`
(`main.rs`, `ServeArgs` struct). Threaded into a new top-level `AppState.disable_update_check:
bool` field (not folded into `AuthConfig`, which is specifically about password/cookie policy — an
update-check suppression flag is an unrelated operational concern and belongs at the same level as
other non-auth `AppState` fields like `library: LibraryPaths`). `misc.rs::server_info`'s
`debug_mode` field reads `state.disable_update_check` directly instead of `flag("devmode", "0")`
against the Redis `LRR_CONFIG` hash.

**Rationale**: This is a direct, mechanical application of an already-established, working pattern
in this codebase (`no_pass` is the closest analog: a boolean that used to gate a whole class of
behavior and is now being retired in favor of deploy-time-only configuration for a *different*
boolean that was never meant to be end-user-configurable in the first place, per spec FR-017 and
the prior conversation's confirmation that `devmode` has zero server-side behavior — see plan.md's
Technical Context).

**Alternatives considered**: A build-time (`cfg!`) flag instead of a runtime CLI flag/env var —
rejected because it would require a distinct build artifact for "update-check-suppressed" vs.
normal deployments, which is a heavier operational burden than a runtime flag for a self-hosted
single-binary deployment model (matching how `no_pass` itself is a runtime, not build-time,
switch).

## 8. Test suite changes

**Decision** (full list, consolidating the prior investigation):

- `crates/lanrurugi-server/tests/settings_toggles.rs`:
  - DELETE `nofunmode_forces_auth_even_when_enable_pass_is_off` (tests a removed concept in full).
  - Simplify `test_app(enable_pass: bool)` helper — password enforcement is no longer
    parameterized, so the helper either drops the parameter entirely or is replaced by a plain
    `test_app()` (exact shape decided at implementation time based on what the two remaining CORS
    tests actually need).
  - ADD: a guest-mode test matrix — guest mode off (unauthenticated → redirect-equivalent
    behavior), guest mode on with zero guest-visible categories (same), guest mode on with one
    guest-visible category (scoped access granted, verified against both an in-scope and an
    out-of-scope archive).
- `crates/lanrurugi-server/tests/contract_api.rs`:
  - `get_info_matches_recorded_serverinfo_shape`: remove the `"nofun_mode"` field assertion
    (lines 225-236 per research); `has_password`/`debug_mode` assertions stay (still present
    fields, per §5/§7).
- `crates/lanrurugi-server/tests/auth_flow.rs`:
  - `session_only_route_rejects_a_real_admin_token_but_accepts_a_real_session`: unaffected, no
    change needed.
  - ADD: a new test verifying an unauthenticated `guest_visitor` request (guest mode on, at least
    one category visible) still gets rejected from a session-only route (e.g.
    `POST /database/drop`) — extends this file's existing pattern to the new role.
- `crates/lanrurugi-api/src/settings.rs` unit test `accepts_a_known_bool_field_and_normalizes_to_1_or_0`
  (line 709-719): swap its example field from `devmode` (removed) to `enablecors` (still present)
  — the test's purpose (verify the generic bool-normalization path) is unaffected by which
  concrete field name it exercises.
- New unit tests: `CategoryRepository` round-trip for `visible_to_guest` (absent-in-Hash → `false`,
  matching `pinned`'s own existing test coverage pattern if one exists, or newly added alongside
  it); `authz::subject_role`'s new `guest_visitor` branch; `SearchParams.restrict_to_archive_ids`
  filtering in isolation (empty set excludes everything, `None` is a no-op, non-empty set narrows
  correctly alongside other active filters).

**Rationale**: Directly derived from the completed code-level investigation (plan.md Technical
Context, "Testing"); no open questions remain here.
