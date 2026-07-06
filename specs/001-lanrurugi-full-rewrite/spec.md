# Feature Specification: LANrurugi — Full Rewrite (Phase 1 Core + Phase 2 Translation)

**Feature Branch**: `001-lanrurugi-full-rewrite`

**Created**: 2026-07-05

**Status**: Draft

**Input**: User description: "Rewrite LANraragi (a self-hosted manga/doujinshi library manager) as LANrurugi, a modern rewrite. Phase 1 must deliver full feature parity with the existing library-management/reading/plugin/API experience while preserving users' existing libraries and fixing a known duplicate-detection defect. Phase 2, delivered later and never blocking Phase 1, adds optional on-page translation with a user-selectable translation backend."

**Scope note**: This specification intentionally spans both delivery phases in a single document so the
relationship between them is explicit. Phase 1 is represented by priorities P1–P3 and is the
blocking, must-ship scope. Phase 2 is represented by priorities P4–P5, is explicitly deferred, and
per project governance MUST NOT delay or gate Phase 1 delivery — see Assumptions.

## Clarifications

### Session 2026-07-05

- Q: Spec uses "a user" in most stories but "an administrator" in the duplicate-repair story
  (now User Story 6 / FR-011, numbered 5 / FR-008 at the time of this question) — does LANrurugi
  introduce a real multi-user/role system, or is this the same single-shared-instance model as the
  previous system? → A: Single-owner model (matches the previous system): one shared
  instance/password, no separate multi-user accounts. "Administrator" denotes the instance owner
  performing an admin-type action, not a distinct account or role.
- Q: FR-005 requires distinguishing archives that share identical leading content — should files
  that are fully byte-identical (e.g. the same file re-scanned, or moved/renamed) still be
  recognized as the same archive? → A: Yes. Byte-identical files remain recognized as the same
  archive; only files that share a leading prefix but differ later must be treated as distinct.
  The fix targets false merges, not deduplication of true duplicates.
- Q: No library-scale assumption was stated anywhere; what size must the system comfortably
  handle? → A: Large personal library: up to approximately 100,000 archives and low-single-digit
  terabytes of content.
- Q: The previous system has a backup/export feature that was not covered by any user story here
  — is it in scope for Phase 1? → A: Yes, in scope. Added as a new User Story 5 (Priority: P2);
  subsequent user stories renumbered accordingly (former User Story 5 → 6, 6 → 7, 7 → 8).
- Q: The previous system has a multi-language UI (`locales/`) that was not covered by any user
  story here — is it in scope for Phase 1? → A: Yes, in scope. Added as a new User Story 7
  (Priority: P3); subsequent user stories renumbered accordingly (former User Story 7 → 8, 8 → 9).

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Continue an existing library without losing anything (Priority: P1)

A user who already runs the previous system points the new system at their existing library
(their previously stored archive records and their archive files on disk) and finds everything —
titles, tags, summaries, categories, groupings, and per-archive reading progress — exactly as they
left it, with no manual re-entry and no separate conversion step required before they can start
browsing.

**Why this priority**: This is the precondition for adoption. If migrating loses or scrambles even
a small fraction of a user's tagging/reading history, nothing else in the rewrite matters — users
will not switch.

**Independent Test**: Point the new system at a library previously managed by the old system, open
it, and verify every archive is listed with its prior tags, categories, groupings, and reading
progress intact, without performing any manual fix-up.

**Acceptance Scenarios**:

1. **Given** a library with existing tagged, categorized archives and partially-read progress
   markers, **When** a user opens that library in the new system for the first time, **Then** every
   archive appears with its previous title, tags, category memberships, and reading-progress state
   unchanged.
2. **Given** an archive that belongs to a multi-volume grouping in the old system, **When** the
   library is opened in the new system, **Then** the grouping and volume order are preserved.
3. **Given** a library that has never been touched by the new system before, **When** it is opened,
   **Then** no archive is duplicated, dropped, or re-numbered as a side effect of the switch.

---

### User Story 2 - New archives are found, catalogued correctly, and never falsely merged (Priority: P1)

A user drops new archive files into their monitored library folder. The files are automatically
detected, catalogued as browsable entries, and — critically — two genuinely different archives
that happen to start with identical content (e.g. the same cover pages) are recognized as two
separate library entries instead of being silently collapsed into one, which was a known problem
in the previous system.

**Why this priority**: Automatic ingestion is core day-to-day value, and the false-merge defect is
one of the concrete, named reasons this rewrite exists — fixing it is not optional polish.

**Independent Test**: Place a new archive file (up to 500MB) in the monitored folder and confirm
it becomes browsable within 60 seconds. Separately, place two different archives that share
identical leading pages but differ later, and confirm both appear as distinct library entries.

**Acceptance Scenarios**:

1. **Given** a monitored library folder, **When** a new valid archive file is added to it, **Then**
   it appears as a new, browsable, searchable library entry without manual action.
2. **Given** two different archive files that share identical leading pages/content but differ
   later in the file, **When** both are ingested, **Then** both appear as separate library entries,
   each independently taggable and trackable.
3. **Given** a file that is still being written/copied into the monitored folder, **When** the
   system notices it, **Then** ingestion waits until the file is stable rather than cataloguing a
   partial/corrupt archive.

---

### User Story 3 - Existing third-party tools keep working (Priority: P2)

A user who already relies on third-party apps/scripts (reader apps, automation scripts, OPDS
clients) that talk to the previous system's API keeps using them after switching to the new
system, with no reconfiguration or update to those tools required.

**Why this priority**: Breaking the existing integration ecosystem would strand users who have
built workflows around it, even though it is not the primary interactive experience.

**Independent Test**: Run an existing third-party client's standard flow (list archives, fetch a
page image, update a tag, run a search, authenticate with an existing API key) against the new
system without modifying the client, and confirm it behaves identically to how it behaved against
the previous system.

**Acceptance Scenarios**:

1. **Given** a third-party client configured with a previously-issued API key, **When** it lists
   archives, fetches pages, or updates tags/categories, **Then** it receives the same shape of
   response it received from the previous system.
2. **Given** an existing automation script that searches the library using the previous system's
   search syntax, **When** it runs against the new system, **Then** it returns equivalent results.

---

### User Story 4 - Automatic metadata enrichment via extensions (Priority: P2)

A user enables one or more metadata-enrichment extensions so that newly added (or existing)
archives automatically get tags, a title, and/or a summary filled in, without the user manually
researching and typing that information for every archive.

**Why this priority**: This existed in the previous system and is a meaningful time-saver for
users with large libraries, but the library is fully usable without it.

**Independent Test**: Enable an extension, trigger enrichment on an archive lacking metadata, and
confirm tags/title/summary are populated from the extension's result; then disable the extension
and confirm the library continues operating normally.

**Acceptance Scenarios**:

1. **Given** an archive with no tags, **When** a metadata-enrichment extension is run against it,
   **Then** the archive's tags/title/summary are updated with the extension's findings.
2. **Given** an extension that fails or hangs while processing one archive, **When** that happens,
   **Then** the failure is isolated to that extension run and reported, and the rest of the system
   (browsing, reading, other extensions) continues operating normally.
3. **Given** an extension that has not been granted a particular capability (e.g. network access
   to a given destination), **When** it attempts to use that capability, **Then** the attempt is
   blocked and reported rather than silently allowed.

---

### User Story 5 - Back up and export the library (Priority: P2)

A user creates a backup/export of their library's metadata — tags, categories, groupings, reading
progress, and configuration — on demand, so their organization work is protected against data loss
and can be restored onto a fresh instance if needed.

**Why this priority**: The previous system already provided this safety net; losing it in Phase 1
would be a real regression for users who rely on it to protect hours of tagging/organization work,
not just missing polish.

**Independent Test**: Trigger a backup/export, then verify the output contains the library's
current tags, categories, groupings, and reading progress, and can be used to restore that state
onto a fresh instance pointed at the same archive files.

**Acceptance Scenarios**:

1. **Given** a library with tagged archives and reading progress, **When** the user triggers a
   backup/export, **Then** the resulting output includes all current tags, categories, groupings,
   and reading progress.
2. **Given** a previously created backup, **When** it is restored onto a fresh instance pointed at
   the same archive files, **Then** the library's tags, categories, groupings, and reading progress
   match the state at backup time.

---

### User Story 6 - Fix historical false duplicates after the fact (Priority: P3)

The owner of a library that was previously affected by the false-duplicate-merging defect runs a
one-time, guided action ("administrator action" here means an action performed by the same single
instance owner — LANrurugi has no separate multi-user/administrator account model, matching the
previous system) that re-evaluates already-catalogued archives and splits apart any that were
incorrectly treated as a single entry, without losing any tags/progress already recorded against
the archive that had been tracked correctly.

**Why this priority**: This repairs pre-existing damage from the defect described in User Story 2.
It matters for affected libraries but is not required for a library encountering the system for
the first time (which is already protected by User Story 2), so it can follow after the P1/P2
scope ships.

**Independent Test**: On a library known to contain a previously-merged pair of archives, run the
repair action and confirm both archives now exist as separate entries, with the originally-tracked
one keeping its existing tags/progress and the previously-hidden one appearing as a newly
recognized entry.

**Acceptance Scenarios**:

1. **Given** a library containing an archive pair that was incorrectly merged into one entry by
   the old defect, **When** the administrator runs the repair action, **Then** the two archives
   become separate, independently taggable entries.
2. **Given** the same repair action, **When** it completes, **Then** the archive that already had
   recorded tags/reading progress keeps that data attached to it, and the previously-hidden archive
   appears as a new, unfilled entry rather than data being lost or scrambled.

---

### User Story 7 - Use the interface in your preferred language (Priority: P3)

A user whose preferred language is not English selects a display language for the interface from
the set the previous system already supported, so menus, labels, and messages appear in that
language rather than only in English.

**Why this priority**: The previous system already shipped this; a large share of its user base
reads in a non-English language day-to-day, so shipping English-only in Phase 1 would be a real
regression for them, not just missing polish. It follows core browsing/reading/compatibility
because the interface is still fully usable in English in the meantime.

**Independent Test**: Switch the interface language to a previously-supported non-English option
and confirm menus, labels, and messages render in that language; confirm any string missing a
translation falls back to English instead of appearing blank or broken.

**Acceptance Scenarios**:

1. **Given** a previously-supported non-English language is selected, **When** the user navigates
   the interface, **Then** menus, labels, and messages appear in that language.
2. **Given** a string that has no translation available in the selected language, **When** it is
   displayed, **Then** it falls back to English rather than appearing blank or broken.

---

### User Story 8 - Verify and demonstrate concurrency/performance gains over the previous system (Priority: P2)

The project maintainer runs a benchmark suite that exercises the same bulk, parallelizable
operations — a full library scan/ingestion of a large synthetic library, and the duplicate-repair
reindex from User Story 6 — against both the previous system and the new system on the same
hardware, and receives a report of wall-clock time and throughput for each, so the rewrite's
concurrency/performance improvement is measured rather than assumed.

**Why this priority**: Weak concurrency was one of the named, concrete motivations for this
rewrite, alongside the duplicate-detection defect (User Story 2). Without a measured comparison
there is no way to confirm that promise was actually delivered rather than assumed from "it's Rust
now."

**Independent Test**: Run the benchmark suite against a synthetic library at the target scale (see
SC-008) on both systems on the same hardware, and confirm a report is produced with concrete
timing/throughput numbers for each measured operation.

**Acceptance Scenarios**:

1. **Given** a synthetic library at the target scale, **When** the benchmark suite is run against
   both systems on the same hardware, **Then** a report is produced showing wall-clock time and
   throughput for full library scan/ingestion and for the duplicate-repair reindex, for each
   system.
2. **Given** the benchmark report, **When** it is reviewed, **Then** it shows the new system
   completing bulk scan/ingestion and reindex operations faster than the previous system on
   multi-core hardware, reflecting improved use of available cores rather than a single-threaded
   bottleneck.

---

### User Story 9 - Optional on-page translation, user's choice of backend (Priority: P4) — Phase 2, deferred

A user reading an archive optionally enables on-page translation and picks which translation
backend to use — a cloud-hosted service or a model running on their own device — so pages render
with translated text without leaving the reader.

**Why this priority**: This is new, valuable capability but is explicitly Phase 2. It is deferred
scope and must not be built or planned in a way that delays User Stories 1–8.

**Independent Test**: With translation disabled, confirm reading behaves exactly as in Phase 1.
With translation enabled and a backend selected, confirm translated text renders on the page being
read.

**Acceptance Scenarios**:

1. **Given** translation is disabled, **When** a user reads any archive, **Then** the experience is
   identical to Phase 1 (no added latency, no new UI, no new failure modes).
2. **Given** translation is enabled with a cloud backend selected, **When** a page is read,
   **Then** the user's credentials for that backend are never visible to or retrievable from the
   browser.
3. **Given** translation is enabled with a backend running on the user's own device, **When** that
   device cannot be reached due to browser/network policy, **Then** the user sees a clear,
   actionable explanation rather than a silent failure.

---

### User Story 10 - Seamless translated reading pace (Priority: P5) — Phase 2, deferred

While reading with translation enabled, upcoming pages are translated ahead of time so the user
does not wait when turning the page, while automatic look-ahead activity against paid backends
stays within a budget the user controls.

**Why this priority**: This is a refinement of User Story 9's experience, valuable but strictly
later and lower-risk to defer than the core translation capability itself.

**Independent Test**: Read forward through several pages with translation and a look-ahead window
enabled, and confirm translated pages are ready by the time the user reaches them; separately,
confirm that with a metered backend selected, look-ahead activity respects a configured limit.

**Acceptance Scenarios**:

1. **Given** translation and look-ahead are enabled, **When** the user turns to the next page
   within the look-ahead window, **Then** the translated version is already available.
2. **Given** a metered (paid) translation backend is selected, **When** the user is reading,
   **Then** automatic look-ahead activity does not exceed the user's configured budget/rate limit.

---

### Edge Cases

- What happens when two archives share identical leading content but differ later in the file?
  (Must be catalogued as distinct entries — see User Story 2.)
- What happens when a file in the monitored folder is only partially written when ingestion
  notices it? (Must wait for it to stabilize rather than catalogue a partial archive.)
- What happens when an administrator runs the duplicate-repair action on a library that turns out
  to have no historical false duplicates? (Must complete as a no-op without altering anything.)
- What happens when an extension attempts an action outside its declared/granted permissions?
  (Must be blocked and reported, not silently allowed or silently dropped.)
- What happens when a user's own local translation backend cannot be reached because of browser
  network-access restrictions? (Must degrade gracefully with an explanation, not fail silently —
  see User Story 9.)
- What happens when the interface is displayed in a non-English language but a plugin/extension
  returns metadata (tags/summary) only in its source language? (The metadata itself is not
  translated by this feature; only interface chrome — menus, labels, messages — is localized.)
- What happens to in-flight look-ahead translation requests when a user navigates away or closes
  the reader before they complete? (Must be abandoned without side effects such as continued
  charges accruing indefinitely in the background.)
- What happens when a third-party client uses an API capability from the previous system that has
  no equivalent behavior change in the new system? (Must behave the same as before; any
  intentional behavior change is a new, separately versioned capability, not a silent change to
  existing behavior.)
- What happens when a backup/export is triggered while the library is actively being modified
  (e.g. mid-scan, or a plugin is writing new tags)? (Must produce a consistent snapshot rather than
  a partially-written or corrupt export.)
- What happens when the benchmark suite is run on hardware with very few CPU cores (e.g. a single-
  core VM)? (Must still complete and report results; the comparison may show a smaller margin, but
  the benchmark itself must not require a specific core count to function.)

## Requirements *(mandatory)*

### Functional Requirements

**Library continuity & data compatibility (supports User Story 1)**

- **FR-001**: System MUST allow a user to open an existing library (its previously stored records
  and its archive files) immediately, without requiring a destructive conversion step before first
  use.
- **FR-002**: System MUST preserve all previously recorded titles, tags, summaries, category
  memberships, multi-volume groupings, and per-archive reading progress exactly as they existed
  before the switch.
- **FR-003**: System MUST recognize archive files already known to the previous system as the same
  library entries they were before, rather than re-importing or duplicating them.

**Archive ingestion & duplicate detection (supports User Story 2)**

- **FR-004**: System MUST automatically detect new archive files placed in a monitored library
  location and add them as browsable, searchable entries without manual intervention.
- **FR-005**: System MUST distinguish between genuinely distinct archives even when they share
  identical leading content, rather than treating them as the same entry. Files that are fully
  identical in content (not merely sharing a leading portion) MUST continue to be recognized as
  the same archive — this requirement targets false merges of differing content, not removal of
  true-duplicate detection.
- **FR-006**: System MUST NOT catalogue a file that is still being written to the monitored
  location; it MUST wait until the file is stable before treating it as a new archive.
- **FR-007**: Duplicate-detection work MUST NOT meaningfully delay or block the ingestion pipeline
  for the common case; heavier verification, if needed, MAY happen after the entry is already
  browsable.

**Library backup & export (supports User Story 5)**

- **FR-008**: System MUST allow a user to trigger, on demand, a backup/export of the library's
  tags, categories, groupings, reading progress, and configuration.
- **FR-009**: The backup/export MUST be usable to restore that same state (tags, categories,
  groupings, reading progress) onto a fresh instance pointed at the same archive files.
- **FR-010**: A backup/export triggered while the library is being actively modified MUST still
  produce an internally consistent snapshot, not a partially-written or corrupt output.

**Duplicate repair (supports User Story 6)**

- **FR-011**: System MUST provide an administrator-triggered action that re-evaluates already
  catalogued archives and splits apart any that were previously merged due to the defect described
  in FR-005's predecessor behavior.
- **FR-012**: The repair action MUST NOT discard tags, summary, or reading-progress data already
  recorded against the archive that had been correctly tracked; the previously-hidden archive MUST
  reappear as a new, distinctly trackable entry rather than reusing/overwriting the existing one.

**Third-party & automation compatibility (supports User Story 3)**

- **FR-013**: System MUST expose the same externally-facing operations (browsing/searching
  archives, fetching pages, managing tags/categories, authenticating via API key) with the same
  request/response contract the previous system exposed, so existing third-party integrations
  continue to function unmodified.
- **FR-014**: Any new externally-facing capability not present in the previous system's contract
  MUST be additive and MUST NOT change the behavior of operations existing integrations already
  rely on.

**Metadata enrichment via extensions (supports User Story 4)**

- **FR-015**: System MUST allow users to enable independently-authored extensions that fetch or
  generate archive metadata (tags, title, summary).
- **FR-016**: A failing, hanging, or misbehaving extension MUST NOT crash, freeze, or degrade the
  core system; its failure MUST be isolated to that extension's run and reported to the user.
- **FR-017**: Extensions MUST only be able to perform categories of action they explicitly declare
  needing; unrequested capabilities MUST be denied by default.

**Interface localization (supports User Story 7)**

- **FR-018**: System MUST allow a user to select a display language for the interface from the set
  of languages the previous system supported.
- **FR-019**: A string with no available translation in the selected language MUST fall back to
  English rather than appearing blank or broken.

**Concurrency benchmarking (supports User Story 8)**

- **FR-020**: System MUST provide a benchmark suite that measures wall-clock time and throughput
  for, at minimum, full library scan/ingestion and the duplicate-repair reindex, at the target
  library scale (see SC-008).
- **FR-021**: The benchmark suite MUST be runnable against both the previous system and the new
  system on the same hardware and produce a comparable report of results for each.
- **FR-022**: Bulk, CPU-bound operations exercised by the benchmark (archive-identity hashing
  during scan/reindex) MUST scale with available CPU cores rather than being limited to
  single-threaded throughput.

**Translation (supports User Stories 9–10, Phase 2 — deferred, non-blocking)**

- **FR-023**: System MUST allow a user to optionally enable on-page translation and choose which
  category of translation backend to use, including at least one locally-hosted option and one
  cloud-hosted option.
- **FR-024**: When a cloud translation backend is used, the system MUST hold the user's credentials
  for that backend server-side only and MUST NOT expose them to, or store them in, the browser.
- **FR-025**: When translation depends on a backend running on the user's own device, the system
  MUST detect when that connection cannot be established and present an actionable explanation
  rather than failing silently.
- **FR-026**: System MUST let users limit automatic look-ahead translation activity for
  metered/paid backends to avoid unexpected usage charges.
- **FR-027**: Disabling translation MUST fully restore the core browsing/reading experience to its
  pre-translation behavior and performance, with no residual latency or resource cost.

### Key Entities

- **Archive**: A single trackable manga/comic work in the library. Has an identity, title, tags,
  summary, page count, thumbnail, reading progress, and optional membership in a category and/or a
  grouping.
- **Category**: A user-defined or rule-based grouping used to organize archives (a fixed list or a
  saved-search-based dynamic grouping).
- **Grouping (multi-volume work)**: A user-defined ordered collection of archives representing
  parts of the same larger work (e.g. volumes of a series).
- **Reading Progress**: Per-archive record of the last position read and completion state.
- **Extension**: An independently-authored unit of logic that can enrich archive metadata or
  perform login/retrieval actions on the user's behalf, with an explicitly declared, limited set of
  permitted capabilities.
- **Translation Backend Selection** (Phase 2): A user's choice of translation provider category
  (local device vs. cloud service) and its non-secret connection details; any credential material
  is handled as a protected reference, never a visible/exportable entity attribute.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: 100% of a migrated library's previously recorded tags, categories, groupings, and
  reading-progress entries are present and correctly attributed the first time a user opens that
  library in the new system, with zero manual data re-entry.
- **SC-002**: A newly added archive file (up to 500MB) becomes browsable and searchable within 60
  seconds of being placed in the monitored location.
- **SC-003**: Two distinct archives sharing identical leading content (e.g. an identical cover) are
  catalogued as separate entries in the overwhelming majority of realistic cases, a substantial
  improvement over the near-total false-merge rate this scenario produced in the previous system.
- **SC-004**: 100% of the previous system's externally-facing operations exercised by existing
  third-party clients complete successfully against the new system without any client-side changes.
- **SC-005**: An administrator can identify and separate a previously-merged duplicate pair in a
  single guided action, with the originally-tracked archive's tags/progress unchanged afterward.
- **SC-006** (Phase 2): A user who never enables translation observes no measurable difference in
  library browsing or reading responsiveness compared to a build without translation at all.
- **SC-007** (Phase 2): When translation and look-ahead are enabled, at least the configured
  look-ahead window of upcoming pages is translated and ready before the user reaches them, and a
  user-set usage budget for metered backends is never exceeded without explicit confirmation.
- **SC-008**: Library browsing, searching, and tag filtering remain responsive with no
  user-perceptible slowdown for libraries containing up to approximately 100,000 archives and
  low-single-digit terabytes of total content.
- **SC-009**: A user can produce a backup/export on demand and use it to restore the library's
  tags, categories, groupings, and reading progress onto a fresh instance, with no discrepancy
  from the state at backup time.
- **SC-010**: A user can switch the interface to any previously-supported non-English language and
  see menus, labels, and messages rendered in that language, with untranslated strings falling
  back to English rather than appearing blank or broken.
- **SC-011**: A published benchmark report shows the new system completing full library
  scan/ingestion and the duplicate-repair reindex faster than the previous system, measured on the
  same multi-core hardware at the target library scale (see SC-008).

## Assumptions

- Users migrating from the previous system have their existing data store and archive files
  reachable from the new system (same host or an equivalent accessible network path).
- The target deployment scale is a large personal library (see SC-008), not a multi-tenant or
  shared community-scale instance; architecture and performance work is scoped accordingly.
- The specific technique used to fingerprint archive content for duplicate detection is an
  implementation decision finalized during planning; the requirement here is behavioral (distinct
  content is recognized as distinct) rather than prescriptive about the exact method.
- "Cloud translation backend" and "locally-hosted backend" denote categories of user choice;
  the specific set of supported providers may expand over time without constituting a scope change
  to this specification.
- The precise set of declarable extension permission categories is an implementation detail
  finalized during planning.
- Phase 2 (User Stories 9–10) is planned and specified here for continuity of vision, but per
  project governance its delivery timeline is independent of, and MUST NOT delay, Phase 1 (User
  Stories 1–8) completion. Planning and task breakdown for Phase 2 MAY proceed separately once
  Phase 1 is stable, without re-opening this specification.
- The benchmark suite (User Story 8) compares wall-clock time/throughput for equivalent
  operations on both systems; it is a verification/reporting deliverable, not a claim about any
  specific numeric speedup factor — the exact margin depends on hardware and library contents.
- Interface localization (User Story 7) covers UI chrome only (menus, labels, messages), not the
  content of user-supplied archive metadata or extension-fetched tags/summaries, which remain in
  whatever language the source data/extension provides.
