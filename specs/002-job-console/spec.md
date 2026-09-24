# Feature Specification: Background Job Console

**Feature Branch**: `002-job-console`

**Created**: 2026-07-07

**Status**: Draft

**Input**: User description: "Add a background job management console (a Minion-admin-console
equivalent) so admins can see all queued/active/finished/failed background jobs (thumbnail
regeneration, backups, rescans, duplicate scans, index rebuilds, etc.), inspect each job's
status/progress/result/error, and clear stale entries — replacing legacy's separate Minion admin
panel now that Minion itself has been consolidated into in-process async tasks per constitution
Principle III. Scoped as an additive Phase 1 task, added after the initial 8 user stories, not a
new phase."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - See what background work is running (Priority: P1)

An admin who just clicked a long-running action elsewhere in the app (e.g. "Regenerate all
Thumbnails" in Settings, or "Scan for duplicates") wants a single place to see every background
job currently queued or running, without having to keep the original page open and watch its own
inline status text.

**Why this priority**: This is the core value of a job console — legacy's own Minion dashboard
exists first and foremost so an admin can confirm "is it still working, or did it die?" without
guessing. Every other capability (detail view, clearing) is secondary to this.

**Independent Test**: Trigger any existing background action (thumbnail regen, backup/restore,
duplicate scan, index rebuild), then open the job console and confirm the job appears with a
live-updating state (queued → active → finished/failed) and progress, without reloading the page.

**Acceptance Scenarios**:

1. **Given** a background job was just queued from another page, **When** the admin opens the job
   console, **Then** that job appears in the list with state "queued" or "active".
2. **Given** a job is actively running, **When** its progress updates, **Then** the console
   reflects the new progress without a manual page refresh.
3. **Given** a job has finished or failed, **When** the admin views the console, **Then** the
   job's final state (finished/failed) is shown and it no longer appears as active.
4. **Given** no jobs have ever run since the server started, **When** the admin opens the console,
   **Then** it shows a clear empty state rather than an error or blank page.

---

### User Story 2 - Inspect what a specific job did or why it failed (Priority: P2)

An admin sees a failed job in the list and wants to know why, or sees a finished job and wants to
confirm what it actually did (e.g. how many thumbnails were regenerated, how many duplicates were
found), without having to reconstruct that from application logs.

**Why this priority**: Visibility into state alone ("failed") isn't actionable — the admin needs
the error message or result payload to decide what to do next. This builds directly on Story 1's
list.

**Independent Test**: Trigger a job that is known to fail (e.g. restore with a malformed backup
file) and one that succeeds, then open each job's detail view and confirm the error message /
result payload is legible and specific enough to act on.

**Acceptance Scenarios**:

1. **Given** a finished job, **When** the admin opens its detail view, **Then** the job's result
   summary (e.g. counts, affected items) is shown.
2. **Given** a failed job, **When** the admin opens its detail view, **Then** the specific error
   message captured at failure time is shown, not a generic "job failed" message.

---

### User Story 3 - Clear old job entries (Priority: P3)

An admin who has been running the server for a while wants to clear finished/failed job entries
out of the list so it isn't cluttered with old history, without that requiring a server restart.

**Why this priority**: A pure convenience/hygiene capability — the console is still useful without
it (Stories 1-2 stand alone), but a list that only ever grows becomes hard to scan over time.

**Independent Test**: With several finished/failed jobs listed, clear them (individually or in
bulk) and confirm they no longer appear, while any still-active job remains visible.

**Acceptance Scenarios**:

1. **Given** one or more finished/failed jobs in the list, **When** the admin clears them, **Then**
   they no longer appear in the console.
2. **Given** a mix of active and finished jobs, **When** the admin clears finished jobs, **Then**
   active jobs are unaffected and continue to be tracked normally.

---

### User Story 4 - Find what you need in a long job list (Priority: P2)

An admin on a server that's been up for a while opens the console to a list that's grown long
(anywhere up to the FR-006 retention bound) and wants to quickly answer "how many jobs failed?" or
"show me just the active ones" or "find the backup job I ran a minute ago" without scrolling
through everything, mirroring the at-a-glance stat bar and filtering legacy's own Minion Admin
dashboard provides.

**Why this priority**: Story 1 already makes every job visible, but "visible" and "findable" are
different problems once the list has more than a handful of entries — this is squarely a
usability concern, not a new capability, so it's independently valuable but not P1-critical.

**Independent Test**: With a mix of queued/active/finished/failed jobs of different names in the
list, confirm the per-state counts shown match reality, filtering by a given state shows only
matching jobs, searching by job name narrows the list accordingly, and the list is paginated
rather than one unbroken scroll.

**Acceptance Scenarios**:

1. **Given** a mix of jobs in different states, **When** the admin views the console, **Then** a
   count of jobs in each state (queued/active/finished/failed) is shown, matching the actual
   counts in the list.
2. **Given** the admin selects a specific state, **When** the filter is applied, **Then** only
   jobs in that state are shown, and the counts from Scenario 1 stay unchanged — they always
   reflect every tracked job, not just the currently filtered/searched subset, matching legacy
   Minion Admin's own stat bar (which likewise doesn't change when you filter its table).
3. **Given** the admin types part of a job's name/type into a search box, **When** the search is
   applied, **Then** only jobs whose name matches are shown.
4. **Given** more jobs exist than fit on one page, **When** the admin views the list, **Then** it
   is paginated (not one unbroken scroll), consistent with FR-006's retention bound making very
   long lists possible.

---

### Edge Cases

- What happens when the admin opens the console while zero jobs have ever run? → Empty state (see
  Story 1, Scenario 4), not an error.
- What happens if the server restarts while jobs are queued/active? → Their history is lost (see
  Assumptions); the console simply shows no prior jobs after restart, the same as a fresh empty
  state. This MUST NOT crash the console or show stale/incorrect state for jobs that no longer
  exist.
- What happens when a very large number of jobs have accumulated (e.g. thousands of small
  per-archive operations over months of uptime)? → The console MUST remain usable (bounded list,
  see FR-006) rather than growing memory usage or page load time unboundedly.
- What happens when two admins have the console open in different browser tabs at once? → Both
  MUST eventually reflect the same job states; exact real-time sync latency is not a hard
  requirement (polling is acceptable, see Assumptions).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST provide a single view listing every background job the server has
  tracked since it last started, showing at minimum: job type/name, current state (queued /
  active / finished / failed), and progress for active jobs.
- **FR-002**: System MUST update a listed job's displayed state and progress as the job
  progresses, without requiring the admin to manually reload the page.
- **FR-003**: System MUST let the admin view a specific job's full result payload (for finished
  jobs) or error message (for failed jobs).
- **FR-004**: System MUST let the admin clear finished and/or failed job entries from the list.
  Clearing MUST NOT be possible for jobs that are still queued or active. The bulk "clear
  finished" action always targets every terminal-state job the server is tracking, independent of
  any state filter or name search (FR-010/FR-011) currently narrowing what's *displayed* — it is
  not scoped to only the currently-visible subset, and the UI MUST make this scope unambiguous
  (e.g. its label should not read as if it only affects the visible rows).
- **FR-005**: System MUST cover every existing background operation that already reports through
  the internal job registry (thumbnail regeneration, database backup/restore, duplicate scans,
  index rebuilds, plugin execution, URL-download ingestion) — no operation already tracked
  internally may be invisible to this console. Note: archive rescans (`POST /shinobu/rescan`) do
  **not** go through the job registry today (verified against
  `crates/lanrurugi-api/src/shinobu.rs` — it's a synchronous operation, not job-tracked), so they
  are correctly out of scope for this requirement unless a future change makes rescans
  job-tracked too.
- **FR-006**: System MUST bound the number of retained job records (oldest finished/failed entries
  evicted first once a limit is reached) so long-running uptime does not cause unbounded memory
  growth or an unusably long list.
- **FR-007**: System MUST present an explicit empty state when no jobs have been tracked, rather
  than an error or indistinguishable blank page.
- **FR-008**: Job console access MUST be subject to the same authentication as the rest of the
  admin-facing UI (no separate, unauthenticated `/minion`-style endpoint) — this is a stricter,
  intentional deviation from legacy for consistency with the rest of the admin surface, not a
  compatibility requirement.
- **FR-009**: System MUST show a count of tracked jobs per state (queued/active/finished/failed),
  matching legacy Minion Admin's own stat bar for the states that apply to this architecture (see
  Assumptions for the two legacy stats — Workers, Locks — that are intentionally not reproduced).
  These counts MUST always reflect every tracked job, regardless of any state filter or name
  search currently applied to the list (FR-010/FR-011) — they are not recomputed over the
  filtered/searched subset.
- **FR-010**: System MUST let the admin filter the job list down to a single state.
- **FR-011**: System MUST let the admin search/filter the job list by job name/type.
- **FR-012**: System MUST paginate the job list rather than rendering an unbroken scroll, given
  FR-006 allows the list to grow up to its retention bound.

### Key Entities

- **Job Record**: A single tracked background operation — its type/name, unique identifier,
  current state, progress (for active jobs), and its result payload or error message once it
  reaches a terminal state. Already exists internally (Phase 1's job registry); this feature makes
  the full collection of records visible and manageable rather than only queryable one-at-a-time
  by callers that already know a specific job's identifier.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An admin can determine whether a just-triggered background action is still running,
  finished, or failed within 5 seconds of opening the console, without consulting server logs.
- **SC-002**: 100% of background operations already exposed through the existing job-tracking
  mechanism are visible in the console (no silent gaps).
- **SC-003**: The console remains responsive (loads and lists jobs without noticeable delay) after
  hundreds of jobs have accumulated during a single server uptime, due to the retention bound in
  FR-006.
- **SC-004**: An admin investigating a failed job can find the specific failure reason without
  needing developer/log access, in a single view (no cross-referencing multiple pages).
- **SC-005**: An admin can find a specific job (by state or by name) in a list of hundreds of
  entries within 10 seconds, using the state filter or name search rather than manually scrolling
  through the entire list.

## Assumptions

- Job history is **not** required to survive a server restart. This matches the existing job
  registry's in-memory design (constitution Principle III explicitly consolidates the legacy
  Minion process into in-process async tasks) and is acceptable because tracked jobs are short,
  admin-triggered actions rather than a durable work queue other systems depend on.
- Near-real-time updates (Story 1) are acceptable via periodic polling from the browser; this does
  not require a push mechanism (e.g. WebSockets/SSE).
- **Retry is out of scope for this feature.** If a job failed, the admin re-triggers the original
  action from wherever they normally would (e.g. the same Settings button) rather than the console
  reconstructing and re-invoking arbitrary job parameters itself. This keeps the console a
  monitoring/inspection surface rather than a generic job-execution engine, avoiding the need to
  persist and generically replay each job type's original invocation arguments.
- The retention bound in FR-006 defaults to a reasonably generous fixed count (e.g. low hundreds)
  rather than a time-based expiry, since exact tuning is an implementation detail, not a
  user-facing behavior.
- This feature depends on the existing internal job-tracking mechanism (Phase 1's job registry)
  already used by backup/restore, thumbnail regeneration, duplicate scans, index rebuilds, plugin
  execution, and URL-download ingestion; it does not introduce a new job-tracking mechanism of its
  own. Archive rescans are intentionally excluded — see FR-005's note.
- **Two of legacy Minion Admin's own stat-bar entries are deliberately not reproduced: "Workers"
  and "Locks".** Verified against `~/LANraragi/lib/LANraragi/Utils/Generic.pm::start_minion` —
  legacy's "workers" are Minion's own worker abstraction, but the underlying process is a
  `Proc::Simple`-spawned **subprocess** of the main LANraragi process; "locks" are Minion's
  distributed advisory-lock primitive for coordinating multiple worker processes. Constitution
  Principle III already consolidates the historically separate Minion worker subprocess into
  in-process async tasks on the single LANrurugi binary's Tokio runtime — there is no separate
  worker process to list, and no multi-process coordination for locks to guard, so both concepts
  are architecturally inapplicable here, not an oversight or a deferred capability.
