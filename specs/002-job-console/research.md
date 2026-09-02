# Research: Background Job Console

## 1. Reuse vs. extend the existing `/api/minion/*` endpoints

**Decision**: Add new, separate endpoints (`GET /api/jobs`, `DELETE /api/jobs/{id}`,
`DELETE /api/jobs?state=finished`) rather than extending `/api/minion/{jobid}` or
`/api/minion/{jobid}/detail`.

**Rationale**: Read `crates/lanrurugi-api/src/jobs.rs` directly — `/api/minion/{jobid}` and
`/api/minion/{jobid}/detail` deliberately mimic legacy LANraragi's own Minion job-status JSON
shape (`task`/`state: inactive|active|finished|failed`/`notes`/`error` for the status endpoint;
plus `args`/`attempts`/`children`/`parents`/`priority`/`queue`/`retries` stub fields on the detail
endpoint) for any third-party tooling or legacy-derived frontend code that polls a specific,
already-known job ID by the legacy contract shape. Constitution Principle II explicitly allows new
LANrurugi-only endpoints without constraining them to legacy's shape — reusing the legacy-shaped
path for a genuinely new capability (listing *all* jobs, which legacy's own contract disclaims any
guarantee about — see the `queue_job` handler's own comment) would conflate two different
contracts and risk breaking the legacy-compatible one under maintenance later. A clean new path
keeps the legacy polling contract untouched and lets the new list/clear shape be whatever is
actually useful for the console (native field names: `id`, `name`, `state`, `progress`, `result`,
`error` — matching `lanrurugi_core::jobs::JobStatus` directly, no legacy-mimicking translation
layer needed since there's no legacy contract to match here).

**Alternatives considered**: Extending `/api/minion/{jobname}/queue`'s sibling space with a
`/api/minion` (no ID) listing route — rejected because it invites exactly the shape confusion
above (is `/api/minion` legacy-compatible or not?) for zero benefit, since nothing legacy-derived
ever calls a bare `/minion` listing endpoint (legacy's own dashboard is server-rendered HTML from
a separate Mojolicious plugin, not a JSON contract other clients could already depend on).

## 2. Job retention ordering & bound (FR-006)

**Decision**: Add an insertion-order-preserving index to `JobRegistry` (an internal `Vec<String>`
of job IDs in creation order, alongside the existing `HashMap<String, JobStatus>`) and a fixed
cap (`MAX_TRACKED_JOBS = 500`) — when `create()` would exceed the cap, evict the oldest job(s) in
a terminal state (`Finished`/`Failed`) first; if no terminal job exists to evict (all 500 slots
somehow active/queued at once — practically unreachable given jobs are short admin-triggered
actions, not a high-throughput queue), the registry temporarily exceeds the cap rather than
dropping an in-flight job's tracking out from under it.

**Rationale**: The existing `by_name` method's own doc comment already flags that `HashMap`
doesn't preserve insertion order and that nothing needed it before this feature. A parallel order
vector is the minimal change that (a) doesn't require switching the whole registry to a different
map type (avoiding unrelated churn in every existing call site) and (b) gives `list_all()` a
natural, stable "most-recent-first" ordering for the console without re-deriving it from UUIDs.
500 is a generous, round number given jobs are coarse admin actions (backups/restores, thumbnail
regen, duplicate scans, index rebuilds, plugin runs, URL downloads) — even frequent manual use
wouldn't realistically approach that count within a single server uptime; exact tuning is
intentionally left as a constant, not a user-facing setting (per spec Assumptions).

**Alternatives considered**: A time-based expiry (e.g. drop jobs older than 24h) — rejected as a
first implementation because it adds a wall-clock dependency and a background sweep task for a
problem a simple bounded-count eviction already solves adequately (per spec Assumptions, exact
tuning is an implementation detail); a count bound is simpler to implement and reason about
correctly under concurrent access.

## 3. Update mechanism (FR-002 near-real-time progress)

**Decision**: Client-side polling (TanStack Query `refetchInterval`, matching the exact pattern
`useShinobuStatus`/`useLogLines` already use elsewhere in `apps/frontend/src/api/hooks.ts`), not a
push mechanism.

**Rationale**: Spec Assumptions already rule out requiring WebSockets/SSE. Polling is the
established pattern for every other "live-ish" admin view in this codebase already (Shinobu
status, log tailing) — reusing it keeps this feature consistent with existing conventions instead
of introducing a new, one-off transport mechanism for a single page.

**Alternatives considered**: Server-Sent Events for true push updates — rejected as unjustified
complexity for an admin-only, low-traffic view where a few seconds of staleness is fully
acceptable (SC-001 only requires determining job state within 5 seconds of *opening* the console,
not sub-second live push).

## 4. Where the console lives in navigation

**Decision**: A new route (`/jobs`) inside the existing authenticated `<Layout>` route group in
`apps/frontend/src/App.tsx`, reachable from the "Open Minion Console" button already present in
`Settings.tsx`'s Background Workers section (currently a stub that shows "not available in this
port" — see prior conversation). No new top-level nav item.

**Rationale**: The button already exists at the exact spot legacy's own `config_shinobu.html.tt2`
puts its "Open Minion Console" action, so wiring it to `navigate('/jobs')` instead of a stub is a
one-line change once the page exists, and keeps parity with legacy's own information architecture
(job console reachable from Settings, not the main nav) without adding to the main nav — which
this project's own prior UI-restoration work already deliberately kept minimal per user feedback.

## 5. Per-state counts, filtering, search, and pagination (US4)

**Decision**: All four capabilities are computed/applied entirely client-side, over the same
`JobRecord[]` array `useJobs()` (US1) already fetches — no new backend endpoint, no query
parameters added to `GET /jobs`.

**Rationale**: `GET /jobs` already returns every tracked job (bounded to `MAX_TRACKED_JOBS = 500`
per §2) in one response; deriving per-state counts is a single pass over an array already in
memory in the browser, and state-filter/name-search/pagination are all just different `.filter()`/
`.slice()` views over that same array. At this data scale (hundreds of records, polled every few
seconds anyway per §3), doing this client-side avoids adding query-parameter parsing, a paging
cursor/offset contract, and a search-matching implementation to the Rust API surface for a problem
JavaScript's own array methods already solve adequately in the browser. This mirrors how legacy's
own Minion Admin dashboard *looks* (a stat bar plus a filterable, paginated table) without needing
to mirror how it's *implemented* (Minion Admin's Vue frontend talks to Minion's own backend
pagination/query API, which exists because Minion is a general-purpose, multi-consumer job queue
library — a concern this single-admin, single-instance console doesn't share).

**Alternatives considered**: Server-side filtering/pagination (`GET /jobs?state=active&page=2`) —
rejected for this version as unnecessary complexity given the list is already capped at 500 and
fully fetched on every poll; would only become worth revisiting if `MAX_TRACKED_JOBS` were raised
substantially or the polling payload size became a real concern, neither of which applies today.
