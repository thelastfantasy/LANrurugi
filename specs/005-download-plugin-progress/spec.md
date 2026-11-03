# Feature Specification: Download Plugin Progress, Concurrency & Rate Limiting

**Feature Branch**: `005-download-plugin-progress`

**Created**: 2026-07-19

**Status**: Draft

**Input**: User description: "Enhance the download-plugin SDK and pipeline to support: (1) optional
progress reporting during a download (downloaded bytes / total bytes), surfaced to the frontend as
a real progress bar on the Jobs page instead of the current 0%->100% jump; (2) optional per-domain
concurrency limits for outbound downloads, configurable with exact-hostname and wildcard rules
(LastPass-style domain rule matching); (3) optional rate limiting for outbound downloads. All
three are opt-in via a new plugin-authored `pluginOptions()` export, which returns the plugin's
configurable defaults plus enough metadata for the frontend to render a settings form; user edits
are persisted and passed back into the plugin/download pipeline on subsequent invocations."

## Clarifications

### Session 2026-07-19

- Q: Should rate limiting be system-wide only, or also configurable per-domain like concurrency? → A: Per-domain, using the same domain-matching rules as concurrency, with a global fallback.
- Q: What should the system-level default concurrency/rate-limit value be when a plugin declares no preference of its own? → A: No system-level default limit at all (unlimited) — but each plugin may declare its own default via `pluginOptions()`.
- Q: When "bundle into one archive" is off and multiple resources are cataloged as separate archives, should they be associated with each other in any way? → A: Reuse the existing category-assignment mechanism (the same one Upload/`download_url` already use) — the user may pick a target category before downloading, and every archive produced by that download (whether bundled into one or kept as several) is added to it.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Real download progress on the Jobs page (Priority: P1)

A user submits a URL to a download plugin (e.g. downloading a manga volume from a supported
site). Today the Jobs page shows the job jump straight from queued to finished with no indication
of how much has downloaded — for a large archive (hundreds of MB), the user has no way to tell if
the download is progressing or stuck. This story makes the download's actual byte progress
visible while it's in flight.

**Why this priority**: This is the single most visible, most requested improvement — it doesn't
require any new configuration UI, works with zero user setup, and directly fixes the most
noticeable gap in the current experience (a large download looking "frozen").

**Independent Test**: Trigger a large-file download via any installed download plugin and watch
the Jobs page; can be fully verified by observing the progress bar advance from 0% to 100% with
intermediate values, and by confirming the finished archive is correctly added to the library —
no concurrency/rate-limit/settings-UI work is needed to deliver or test this story.

**Acceptance Scenarios**:

1. **Given** a user has triggered a download via a download plugin, **When** the download is in
   progress, **Then** the Jobs page shows a progress bar reflecting actual bytes downloaded out of
   the total expected size (when the total size is known upfront).
2. **Given** a download's total size cannot be determined in advance (e.g. the server doesn't
   report content length), **When** the download is in progress, **Then** the Jobs page shows an
   indeterminate/spinner-style progress indicator rather than a stalled or misleading percentage.
3. **Given** a download plugin resolves multiple individual resources into one final archive (a
   multi-page artwork bundled into a single manga file), **When** any of those resources is being
   downloaded, **Then** the combined progress across all resources is reflected as one continuous
   progress indicator for that job, not per-resource bars.
4. **Given** a download fails partway through (network error, server error), **When** the failure
   occurs, **Then** the job is marked failed with a clear reason, and any partially-downloaded data
   is not left behind as a broken or half-cataloged library entry.

---

### User Story 2 - Limit simultaneous downloads per source site (Priority: P2)

A user (or, by default, the system on the user's behalf) wants to avoid hammering a single source
website with too many simultaneous download requests — some sites rate-limit or temporarily block
clients that open too many concurrent connections, which can cause an entire batch of downloads
from that site to fail. This story lets concurrency be capped per source, using rules that can
target either one exact site or a whole family of related subdomains at once.

**Why this priority**: Protects users from a real, observed failure mode (getting blocked by a
source site) once progress reporting (Story 1) has made bulk/batch downloading more visible and
therefore more likely to be used for larger batches — but a sensible built-in default already
protects users who never touch this setting, so it's not blocking for basic usability.

**Independent Test**: Configure a low per-domain concurrency limit for a source, trigger several
downloads from that source at once, and observe (via the Jobs page and/or download timing) that no
more than the configured number run against that domain simultaneously, while downloads from other
domains are unaffected.

**Acceptance Scenarios**:

1. **Given** a download plugin declares a default concurrency limit for the domain(s) it downloads
   from, **When** multiple downloads targeting that domain are triggered around the same time,
   **Then** no more than the configured number of downloads to that domain run at the same time —
   the rest wait their turn rather than failing outright.
2. **Given** a user sets a custom concurrency rule for a domain pattern (e.g. "all subdomains of a
   given site share one combined limit"), **When** downloads target different subdomains matching
   that pattern, **Then** they are all counted together against that one shared limit.
3. **Given** both a wildcard rule and an exact-hostname rule could apply to the same download
   target, **When** the system decides which limit governs it, **Then** the exact-hostname rule
   takes precedence over the wildcard rule.
4. **Given** a user has not configured any custom concurrency settings, **When** downloads run,
   **Then** the plugin's own built-in default limits apply automatically with no setup required.

---

### User Story 3 - Cap download bandwidth usage (Priority: P3)

A user wants to prevent large downloads from saturating their network connection (e.g. while they
or others are using the same connection for other things), by optionally capping how fast
downloads are allowed to run — either for one specific source, a family of related sources, or as
an overall ceiling covering everything.

**Why this priority**: A quality-of-life control valued by a subset of users with constrained
connections or shared networks; it's independently useful without either of the other two stories
but has a narrower audience than progress visibility or anti-block protection, so it's lowest
priority.

**Independent Test**: Set a rate limit (for a specific domain pattern, or as a general fallback),
trigger a download of a reasonably large file from a matching source, and confirm (via elapsed
time vs. file size, or the live progress indicator from Story 1) that the observed download speed
does not meaningfully exceed the configured cap.

**Acceptance Scenarios**:

1. **Given** a user has set a rate limit for a specific domain pattern, **When** a download from a
   matching domain runs, **Then** its observed transfer speed stays at or below that domain's
   configured cap.
2. **Given** a user has set only a general (non-domain-specific) rate limit, **When** a download
   runs against a domain with no domain-specific rate limit of its own, **Then** the general cap
   applies as a fallback.
3. **Given** no rate limit has been configured at all (no domain-specific rule and no general
   fallback), **When** a download runs, **Then** it proceeds at full available speed exactly as it
   does today.

---

### User Story 4 - Configure a download plugin's behavior from the UI (Priority: P2)

A user wants to review and adjust a download plugin's concurrency/rate-limit defaults, and — for
plugins that assemble a result out of multiple downloaded pieces — whether those pieces should be
combined into one final file or kept as separate library entries, without editing any files or
touching a terminal.

**Why this priority**: Without a UI, Stories 2 and 3's per-domain/rate-limit customization would
only be reachable by users comfortable editing raw configuration directly, which defeats the
purpose of making these settings user-facing at all — but the settings still have safe built-in
defaults, so this doesn't block Story 1 from shipping independently.

**Independent Test**: Open a download plugin's settings from the plugin management UI, change a
value, save, and confirm (by re-opening the settings, and by observing the next download's actual
behavior) that the change persisted and took effect.

**Acceptance Scenarios**:

1. **Given** a download plugin exposes configurable options, **When** a user opens that plugin's
   settings, **Then** they see a form reflecting the plugin's current effective settings (custom
   value if set, otherwise the plugin's own built-in default) with a human-readable description of
   what each setting does.
2. **Given** a user changes a setting and saves it, **When** that plugin is next used, **Then** the
   new setting takes effect without requiring any restart or reinstall.
3. **Given** a download plugin declares no configurable options at all, **When** a user views that
   plugin in the management UI, **Then** no settings form is shown for it (nothing to configure).
4. **Given** a user enters an invalid value in a settings field (e.g. a negative rate limit), **When**
   they attempt to save, **Then** the system rejects the invalid value with a clear explanation
   rather than silently accepting or discarding it.

### Edge Cases

- What happens when a download plugin doesn't report a `Content-Length`/total size at all? →
  Covered by Acceptance Scenario 2 of Story 1 (indeterminate progress, not a stuck percentage).
- What happens when the *plugin process itself* fails or times out before it ever hands off a
  downloadable resource (e.g. it can't resolve a valid link at all)? → Existing failure handling is
  unchanged; this feature only adds visibility/limits to the actual byte-transfer phase that comes
  after a plugin has already produced something downloadable.
- What happens if a user sets a per-domain concurrency limit or rate limit so low that a legitimate
  batch of downloads takes an impractically long time? → This is the user's own explicit choice;
  the system does not need to warn about or override a deliberately conservative setting.
- What happens when two different installed download plugins both declare settings for the exact
  same domain? → Each plugin's settings apply independently to its own downloads; this feature does
  not need to unify or deconflict settings across different plugins targeting the same site.
- What happens to an in-progress download's progress/limit settings if a user changes the
  plugin's configuration mid-download? → The change takes effect for downloads started after the
  change; an already-running download keeps the settings that were in effect when it started.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST show live, incrementally-updating progress (not just queued →
  finished) for a download-type background job, based on actual bytes transferred.
- **FR-002**: The system MUST show an indeterminate progress state (rather than a fixed or
  misleading percentage) when a download's total size cannot be determined in advance.
- **FR-003**: When a download plugin's result is assembled from multiple individually-downloaded
  resources, the system MUST present combined progress across all of them as a single indicator
  for that job, not one indicator per resource.
- **FR-004**: The system MUST mark a download job as failed, with a human-readable reason, if the
  transfer fails partway through, and MUST NOT leave a partially-downloaded result cataloged in
  the library as if it were complete.
- **FR-005**: The system MUST allow a download plugin to declare a default limit on how many of
  its downloads may run at the same time against a given source domain.
- **FR-006**: The system MUST allow a user to override a download plugin's concurrency limit for a
  specific domain pattern.
- **FR-007**: Domain-pattern matching for concurrency rules MUST support both an exact hostname and
  a wildcard pattern covering a family of subdomains, and an exact-hostname match MUST take
  precedence over a wildcard match when both could apply to the same target.
- **FR-008**: The system MUST apply a download plugin's built-in default concurrency limits
  automatically when a user has not configured any custom override; if a plugin declares no
  default of its own and the user has set no override, the corresponding domain has no
  concurrency limit at all (unlimited) — the system itself imposes no fallback ceiling.
- **FR-009**: The system MUST allow an optional rate limit to be configured that caps download
  transfer speed, using the same domain-pattern matching (exact hostname or wildcard, exact taking
  precedence) as concurrency rules, plus a general (non-domain-specific) rate limit usable as a
  fallback for domains with no rate limit rule of their own.
- **FR-010**: When no rate limit applies to a given download — no matching domain-specific rule and
  no general fallback configured — downloads MUST proceed at unrestricted speed exactly as they do
  today.
- **FR-011**: The system MUST let a download plugin declare which of its settings (concurrency
  default, rate limit default, and — for plugins that assemble multiple downloaded resources into
  one result — whether to bundle them into a single library entry or catalog each separately) are
  user-configurable, along with a human-readable description of each.
- **FR-012**: The system MUST let a user view a download plugin's current effective settings
  (showing the user's own override if one exists, otherwise the plugin's declared default) through
  a settings interface, without needing to edit any file or use a command line.
- **FR-013**: The system MUST persist a user's setting changes for a plugin so they remain in
  effect across future downloads until explicitly changed again.
- **FR-014**: The system MUST reject an invalid setting value (e.g. a negative or nonsensical
  concurrency/rate-limit number) at the time a user attempts to save it, with a clear explanation,
  rather than silently discarding or accepting it.
- **FR-015**: The system MUST NOT show a settings interface for a download plugin that declares no
  configurable options.
- **FR-016**: A user's mid-download change to a plugin's settings MUST NOT retroactively alter an
  already-in-progress download — the change takes effect for downloads started afterward.
- **FR-017**: The system itself MUST NOT impose any built-in default concurrency limit or rate
  limit — a domain with no plugin-declared default and no user override is unlimited; a plugin MAY
  declare its own default concurrency/rate-limit values for the domain(s) it targets, and that
  plugin-declared default applies until a user overrides it (see FR-008).
- **FR-018**: When a download plugin resolves into multiple individually-cataloged archives (the
  plugin's `bundle_as_archive`-equivalent setting is off), the system MUST support associating all
  resulting archives with a single category the user selects before the download starts, using the
  same category-assignment mechanism already available for manual archive uploads.

### Key Entities

- **Download Job**: A background unit of work representing one user-triggered download request
  (which may resolve to one or many individually-fetched resources). Tracks its overall state,
  combined progress (bytes transferred / total bytes when known), and failure reason if
  applicable.
- **Download Plugin Settings**: The set of user-configurable values for one installed download
  plugin (concurrency limit(s) by domain pattern, rate limit, and — where applicable — whether to
  bundle multiple fetched resources into one library entry). Has a plugin-declared default state
  and an optional user override, kept separately so the plugin's own default remains recoverable.
- **Domain Rule**: A single rule pairing a domain pattern (exact hostname or wildcard covering
  related subdomains) with a maximum number of simultaneous downloads and/or a transfer-speed cap
  permitted against matching domains. A general (non-domain-specific) rate-limit fallback is a
  degenerate case of this same concept with no domain pattern attached.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user watching the Jobs page during a download of at least 50MB can see the
  progress indicator advance through at least 3 distinct intermediate states between 0% and 100%,
  not just a single jump from queued to finished.
- **SC-002**: When a source site would otherwise reject or fail requests beyond N simultaneous
  connections, a batch of downloads from that site completes with zero connection-related failures
  once the corresponding concurrency limit is set to N or below.
- **SC-003**: A user can locate, view, and successfully change a download plugin's settings, with
  the change reflected in that plugin's next download, in under 60 seconds, without consulting any
  documentation outside the settings screen itself.
- **SC-004**: Existing download plugins continue to function with zero required changes to
  end-user workflow (no double-configuration, no broken downloads) once this feature ships — a user
  who never opens the new settings screen sees no behavior change beyond the new progress bar.
- **SC-005**: With a rate limit configured, sustained download throughput for a large file stays
  within 10% of the configured cap, measured over the download's full duration.

## Assumptions

- Existing download plugins (and any future ones) are expected to be updated to report the
  information needed for real progress tracking and byte-level transfer; a plugin that doesn't
  provide this falls back to today's behavior (jump from queued to finished) for progress purposes
  only — concurrency and rate-limit controls still apply to it as long as it identifies what it's
  downloading and from where.
- "Domain" for concurrency-rule and rate-limit purposes means the hostname portion of a download's
  source URL; matching is case-insensitive and does not consider port or scheme.
- The system itself imposes no default concurrency or rate limit — an unconfigured domain is
  unmanaged (unlimited) unless the plugin targeting it declares its own default (see FR-017);
  "unmanaged" here is an accepted, deliberate default, not a gap to be filled later.
- Settings changes apply per-plugin, not globally across all download plugins at once — each
  plugin's configuration is independent, matching how each targets different sources with
  different needs.
- No login/authentication concept beyond what already exists is required to view or change these
  settings — the existing admin-facing plugin management screens are the intended home for this
  configuration UI.
- The category selected before a download starts (an existing mechanism already used by manual
  archive upload and the current `download_url` flow) is the mechanism used to associate multiple
  separately-cataloged archives from one non-bundled download together — no new grouping concept
  is introduced.
