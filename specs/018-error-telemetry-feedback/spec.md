# Feature Specification: Error Telemetry and Feedback Bundles

**Feature Branch**: `018-error-telemetry-feedback`

**Created**: 2026-10-28

**Status**: Draft

**Input**: User description: "把 /logs 页面对用户没什么用的现状换掉：在设置里提供一个诊断与反馈的入口，能按时间/级别/来源筛选错误与日志、逐条或按区间挑选，把选中的服务端日志 + 前端数据 + 环境信息打包成 zip，并自动生成一段含标题和正文的反馈文本，方便用户自己发到 GitHub issue。同时希望做到 Sentry 那样的效果但不依赖 Sentry 这类 SaaS：前后端都自动捕获错误并带上下文（breadcrumbs、请求 id、release），本地先存、本地看、本地导出；如果管理员配置了自托管的 Sentry 兼容追踪服务（例如 Rustrak），可以在弹窗里临时开启一个 5 分钟 / 1 小时 / 1 天的时间窗把这段时间的数据发过去，窗口到期或手动关闭后立刻停止外发。版本标识用 git sha 做 release，源码映射按 release 保存在 CI 产物里、按需取用，默认不把任何映射文件放进用户导出包。"

## Context

`/logs` today is a legacy-shaped page: five fixed categories, "last N lines" of one file at a time, no
level/time/source filtering, no correlation to the action that failed, and no artifact a user can hand
to anyone. Meanwhile an error in a self-hosted deployment is hard to report: the administrator has to
guess which category and which minute to copy, and the maintainer has to guess a version.

This feature replaces that with three connected pieces:

1. **Automatic capture, locally kept.** Both the server and the web client capture errors without any
   user action, keep them in a bounded local store, and group them into issues the administrator can
   read in the settings page. This works on an intranet with no outbound access at all.
2. **A feedback bundle the user builds.** From the same store, the administrator selects a time range
   and/or individual entries plus the kinds of supporting data wanted (server logs, frontend events,
   environment/diagnostics), previews the result, and gets a zip plus a prefilled feedback text
   (title + body) that can be attached to a GitHub issue. Nothing is ever uploaded by the app itself.
3. **Optional, time-boxed forwarding.** An administrator may point the instance at a self-hosted,
   Sentry-protocol-compatible tracker (for example Rustrak) and open a short sending window. Only
   events inside that window leave the instance; the window closes itself on expiry and can be closed
   at once, and an instance with no configured endpoint never sends anything.

Every report carries the build identity (`release`, i.e. the git sha) so the stack can be resolved
later; source maps are stored per release as build artifacts and are not part of a user's bundle by
default.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Errors are captured and readable without any external service (Priority: P1)

An administrator whose instance has no outbound network access hits a failure in the web UI or the
server. Without doing anything, the failure — with its stack, breadcrumbs and request identity —
appears in the settings page as one entry in a grouped issue, and can be opened to read what happened,
in which release, and how often.

**Why this priority**: this is the whole point of replacing `/logs`. If it only worked with a remote
tracker configured, the intranet deployments (the common case for this project) would get nothing.

**Independent Test**: on a machine with networking disabled, trigger a server error and a browser
error, then open the settings page: both are present, grouped, and readable, and the local store's
retention/cap behaviour can be observed by filling it.

**Acceptance Scenarios**:

1. **Given** an instance with no telemetry endpoint configured, **When** an unhandled exception occurs
   in the web client, **Then** an issue appears in the settings page within seconds, showing the
   message, the stack, the release, and the breadcrumbs leading to it.
2. **Given** the same instance, **When** the server panics or returns a 5xx for a request, **Then** an
   issue appears with the server-side stack and the same request identity the browser saw.
3. **Given** an issue that repeats, **When** it happens again, **Then** the existing issue's count and
   last-seen time update rather than a second issue being created.
4. **Given** the store is full, **When** a new error arrives, **Then** the oldest entries are evicted,
   the new error is still recorded, and the settings page reports how many were dropped.

---

### User Story 2 - Building a feedback bundle and text for a GitHub issue (Priority: P1)

The administrator selects a time range (or individual entries) covering the problem, chooses to
include server logs, frontend events and environment details, previews the file list and size, and
gets a zip plus a feedback text with a title and a body that already names the version, the release,
the request identity and what the bundle contains. They then open a GitHub issue themselves and
attach the zip.

**Why this priority**: this is the user-facing replacement for the `/logs` page; it is what makes a
report actionable and it needs no server.

**Independent Test**: select a one-hour window containing a known error, generate the bundle, unzip it,
and confirm every file is present, scrubbed as configured, and that the feedback text mentions the
same request identity as the issue detail.

**Acceptance Scenarios**:

1. **Given** at least one captured event, **When** the administrator picks a time range and generates
   a bundle, **Then** the zip contains the selected server log lines, the selected frontend events,
   the environment/diagnostics file and a README explaining each file.
2. **Given** the bundle generation form, **When** the administrator deselects "frontend events",
   **Then** the zip does not contain them and the feedback text says so.
3. **Given** the list filtered to errors, **When** the administrator selects all rows, then excludes
   one, **Then** the selection count and size follow, and the bundle contains exactly the rest.
4. **Given** a generated bundle, **When** the administrator switches the feedback text's language,
   **Then** the title and body are available in both languages and either can be copied.
3. **Given** a bundle larger than the configured cap, **When** it is generated, **Then** the oldest or
   least useful lines are truncated, the zip stays within the cap, and the README states what was
   truncated.
4. **Given** any bundle, **When** it is produced, **Then** it contains no share codes, tokens, cookies,
   API keys, absolute paths or archive titles/filenames unless the administrator explicitly opted in
   to including archive names.
5. **Given** a generated bundle, **When** the administrator copies the feedback text, **Then** the text
   names the version, the release, the platform, the selected range, the request identity and the
   bundle's file name.

---

### User Story 3 - Temporarily forwarding events to a self-hosted tracker (Priority: P2)

An administrator who runs a Sentry-compatible self-hosted tracker (for example Rustrak) enters its
address and project key once, then opens a sending window of five minutes, one hour or one day. The
settings page shows the remaining time and a stop button. While the window is open, errors also go to
that tracker, where they are grouped and symbolized like any other Sentry client. When the window
expires it closes by itself, including after a page reload, and nothing more is sent.

**Why this priority**: it delivers the "Sentry-like" experience without requiring a tracker for
everyone; it is separable from P1 and P2 by design.

**Independent Test**: point the instance at a test tracker, open a five-minute window, provoke two
errors, confirm both arrive at the tracker, close the window, provoke a third, confirm it stays local
and the tracker received nothing more.

**Acceptance Scenarios**:

1. **Given** a configured endpoint and a closed window, **When** an error occurs, **Then** it is stored
   locally and nothing is sent to the endpoint.
2. **Given** a configured endpoint, **When** the administrator opens a one-hour window, **Then** the
   settings page shows the remaining time and events from that moment are also accepted by the
   tracker.
3. **Given** an open window, **When** it expires or the administrator presses stop, **Then** no event
   is sent afterwards — verified by the tracker receiving nothing for at least two minutes during
   continued error generation.
4. **Given** an open window and an unreachable endpoint, **When** errors occur, **Then** they are still
   stored locally, the window shows delivery failures instead of silently dropping them, and it does
   not extend itself.
5. **Given** the browser is reloaded while a window is open, **When** the page comes back, **Then** the
   remaining time reflects the original deadline rather than restarting it.

---

### User Story 4 - Reports stay readable across releases (Priority: P2)

An error reported from an older build keeps its identity: it names the release (git sha), so the stack
can be resolved against that release's source maps later — either by the instance itself when the maps
for that release are available, or by the maintainer from the release's build artifacts. Bundles never
carry the whole map set.

**Why this priority**: without it a minified client stack is unreadable and every report costs a round
trip of guessing. It is independent of the forwarding story because it works for local bundles too.

**Independent Test**: report an error from build A, then generate a bundle while the maps for build A
are available and confirm the stack frames are resolved to file/line/function; then make the maps
unavailable, regenerate, and confirm the frames stay raw but the release is still stated.

**Acceptance Scenarios**:

1. **Given** an error from a client build, **When** the issue is opened, **Then** it states the release
   and the chunk/file where the failure happened.
2. **Given** the maps for that release are available to the instance, **When** a bundle is generated,
   **Then** the resolved frames appear in the bundle and the map files themselves are not included.
3. **Given** the maps for that release are unavailable, **When** a bundle is generated, **Then** the raw
   frames are included together with the release and chunk identity, and the README says how to
   resolve them.
4. **Given** any release, **When** its maps are requested on demand, **Then** only the chunks actually
   referenced by the selected events are fetched, and the transfer is bounded and observable.

---

### User Story 5 - An offline bundle can still reach the tracker later (Priority: P3)

An administrator on an intranet collects a bundle, carries it to a connected machine, and uploads the
bundle's events to the same tracker later, without re-triggering the error.

**Why this priority**: it closes the loop for air-gapped instances, but a full workflow already exists
without it (attach the zip to an issue).

**Independent Test**: generate a bundle with networking disabled, then import its events into a test
tracker and confirm the issues appear with the original timestamps and releases.

**Acceptance Scenarios**:

1. **Given** a bundle produced offline, **When** it is imported into a tracker, **Then** the tracker
   shows the same errors, releases and timestamps.
2. **Given** an import that the tracker rejects, **When** it fails, **Then** the failure is reported to
   the administrator without deleting the bundle.

---

### Edge Cases

- An error occurs while the local store is unavailable: the application must not fail or
  slow down because of telemetry; the error is dropped and counted as a drop.
- A single event carries a huge stack, many breadcrumbs or a large response body: it is truncated to
  the event size cap, and the issue notes the truncation.
- An error repeats rapidly: identical fingerprints within a short window are coalesced into one stored
  event with an increased count instead of filling the store.
- The administrator's clock and the server clock disagree: events carry both the client and server
  timestamps plus the measured offset, so a bundle's timeline remains coherent.
- A window is open when the server restarts: the window does not silently extend or reopen; it is
  either resumed from its stored deadline or closed.
- Multiple browser tabs: the window and the local store are instance-level, not per tab.
- The user is not an administrator: capture still happens (it is not a user action), but issues, the
  bundle builder and the endpoint settings are administrator-only.
- An archive title, share code or token appears in a message, stack or breadcrumb: it is scrubbed
  before storage, not only before export, so the local store never holds it.
- A release's maps are large: fetching them happens only for the chunks referenced by the selected
  events, never for the whole build.
- A bundle is requested while events are still arriving: the selection is frozen at generation time and
  the bundle says so.

## Requirements *(mandatory)*

### Functional Requirements

**Capture and storage (local-first)**

- **FR-001**: The system MUST capture errors automatically on both sides without user action: uncaught
  exceptions, unhandled rejections, render failures and failed requests (4xx/5xx) in the web client;
  panics, error-level log events, failed jobs and refused share operations on the server.
- **FR-002**: Every captured event MUST carry at minimum: a timestamp (client and server), level,
  source, message, stack (when available), release, platform, the request identity when it belongs to
  a request, and the breadcrumbs leading to it.
- **FR-003**: The system MUST group events into issues by a deterministic fingerprint computed from the
  error's own fingerprint when supplied, otherwise from its type, the top user frame and a normalized
  message; the same input MUST always produce the same issue.
- **FR-004**: Each issue MUST expose its count, first and last seen time, the releases it appeared in,
  its level and whether it is resolved or ignored; an administrator MUST be able to mark an issue
  resolved or ignored.
- **FR-005**: Events and issues MUST be stored locally with bounded retention, evicting the oldest
  first; the administrator MUST be able to set any of the three limits — a retention duration
  (default 7 days), an event count (default 50,000) and a storage quota (default 512 MB) — and the
  first one reached MUST trigger eviction. The defaults apply until changed, changes take effect
  immediately without discarding the store, and the current usage plus the number of events dropped
  for retention or rate limits MUST be reported in the settings page.
- **FR-006**: Ingest MUST be rate limited per instance (default: 60 events per minute and 1,000 per
  hour), and a single event MUST be capped in size (default: 64 KB) with truncation noted.
- **FR-007**: Telemetry MUST never make a user-facing operation fail: if the store, the network or the
  tracker is unavailable, the operation proceeds and the event is counted as dropped or pending.

**Privacy and consent**

- **FR-008**: Events MUST be scrubbed before storage and before any export or forwarding: share codes
  (both the readable form and the sealed route suffix), recipient and API tokens, cookies,
  authorization headers, API keys and absolute filesystem paths MUST be removed; archive titles and
  filenames MUST be excluded by default behind an explicit opt-in, with only an identifier prefix kept
  otherwise.
- **FR-009**: The system MUST provide a global on/off switch for capture and per-category switches for
  frontend events, server events, breadcrumbs and request bodies, each effective immediately.
- **FR-010**: An instance with no configured telemetry endpoint, or with the sending window closed,
  MUST NOT transmit any event, bundle or diagnostic data anywhere.
- **FR-011**: The app MUST never upload a bundle or a report on its own; the only outbound path is the
  configured endpoint while a window is open.

**Feedback bundle and text**

- **FR-012**: An administrator MUST be able to select material for a bundle from the merged timeline
  (captured events and server log lines together) by time range, by level, by source and by individual
  entry, with select-all / invert / "only errors" helpers over the current filter; the selection MUST
  show its own count and estimated size. They MUST also choose which supporting categories are
  included: server logs, frontend events, environment/diagnostics, configuration.
- **FR-013**: Before generating, the system MUST show the file list to be produced and an estimated
  size, and MUST refuse a bundle that would exceed the configured cap (default: 20 MB) while offering
  the truncation that would fit.
- **FR-014**: A generated bundle MUST be a single archive containing the selected server log lines
  (continuation lines kept with their entry), the selected frontend events, the environment and
  diagnostics data, the manifest describing the selection and the scrubbing applied, and a README.
- **FR-015**: The system MUST generate a feedback text alongside the bundle, in both Simplified
  Chinese and English, each with a title line and a body: the version, release, engine version,
  platform, the selected range and counts, the most frequent selected issue's message and
  fingerprint, the request identity when present, the categories included, and the bundle's file
  name. The user MUST be able to edit the title before copying, and to copy either language in one
  action.
- **FR-016**: Bundle generation MUST NOT block the application and MUST report progress for bundles
  that take longer than one second.
- **FR-017**: The bundle's events MUST keep the standard error-reporting envelope form, so the same
  archive can be imported into a compatible tracker later without re-triggering the error.

**Optional forwarding (time-boxed)**

- **FR-018**: An administrator MUST be able to configure an endpoint (address plus project key) for a
  self-hosted, standard-protocol error tracker; any tracker that accepts the standard envelope and
  needs no other configuration MUST work by address/key alone.
- **FR-019**: An administrator MUST be able to open a forwarding window of 5 minutes, 1 hour or 1 day,
  see the remaining time, and stop it at once; the window MUST close automatically at its deadline,
  including across page reloads and server restarts, and MUST NOT extend itself on restart.
- **FR-020**: While the window is closed, no event MAY be forwarded; while it is open, events MUST be
  forwarded and additionally kept locally.
- **FR-021**: Forwarding failures MUST be visible (count and last error) and MUST NOT drop the local
  copy or block capture.
- **FR-022**: The endpoint configuration and the window MUST be administrator-only.

**Release identity and symbolication**

- **FR-023**: Every event MUST carry the release identity (build commit) and the software version, and
  the engine version for share-related events.
- **FR-024**: A bundle MUST NOT contain source-map files by default; when the maps for the reported
  release are available to the instance, the stack frames MUST be resolved before packaging; when they
  are not, the raw frames plus the release and file identity MUST be included with an explanation.
- **FR-025**: Source maps MUST be stored per release (as build artifacts of that release) and fetched
  on demand for the chunks referenced by the selected events only, with a bounded transfer and an
  observable failure when unavailable.
- **FR-026**: A resolved frame MUST state its file, line and, when available, the function name; the
  resolution MUST never block the error's capture or the settings page.

**Access and UX**

- **FR-027**: Issues, the bundle builder, the endpoint configuration and the window MUST be reachable
  only by administrators and MUST NOT be exposed to guests or restricted visitors.
- **FR-028**: The settings page MUST show, in one place: capture on/off, the current store usage and
  retention, the drop counters, the endpoint and window state with remaining time, and the entry to
  the issue list and the bundle builder.
- **FR-029**: The issue list MUST show, for every row, its level (info / warning / error), its message
  and its timestamp(s), and MUST support filtering by level, time range, source, release and free text,
  plus sorting by most recent and most frequent. Rows MUST be multi-selectable with the same
  select-all / invert / "only errors" helpers, and the current selection's count and estimated size
  MUST be visible. The list MUST remain usable at the full retention volume.
- **FR-030**: The existing `/logs` page MUST remain available for raw inspection but MUST no longer be
  the primary path: it is linked from the diagnostics settings, not from the main navigation.

### Key Entities

- **Error event**: one captured occurrence — timestamp (client and server), level, source, message,
  stack, breadcrumbs, request identity, release, platform, tags, truncation markers.
- **Issue**: a deterministic group of events — fingerprint, count, first/last seen, releases, level,
  resolved/ignored state, representative event.
- **Breadcrumb**: a step leading to an event — timestamp, category (navigation, interaction, request,
  state), short description, no form values.
- **Request identity**: a short opaque identifier attached to a request, present in its logs, its error
  response and the browser's own record of the failure.
- **Feedback bundle**: a generated archive — manifest, selected server logs, selected frontend events,
  environment/diagnostics, README, plus the feedback text produced alongside it.
- **Telemetry endpoint**: an administrator-provided tracker address and project key, with the state of
  the forwarding window (closed, open with deadline, stop requested).
- **Release maps**: the source-map set belonging to one release identity, stored per release and
  fetched per referenced chunk.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: On an instance with no outbound network access, an error raised in the browser or on the
  server is visible in the settings page within 5 seconds, with no user action.
- **SC-002**: 100% of the feedback workflow (select → preview → generate → copy text) works with no
  network access at all.
- **SC-003**: A repeated error occurring 100 times in 24 hours produces exactly one issue whose count
  is 100, and the store's retained event count stays bounded.
- **SC-004**: With a configured tracker and an open 5-minute window, the first error reaches the
  tracker within 10 seconds; after expiry or a manual stop, zero events reach it over the following
  two minutes of continued error generation.
- **SC-005**: A bundle generated for a one-hour window is produced in under 30 seconds for the default
  retention volume, stays within the size cap, and contains no share code, token, cookie, API key,
  absolute path or archive title/filename that was not explicitly opted in.
- **SC-006**: For a client error, the reported stack identifies the file and line in the original
  source in at least 90% of cases where that release's maps are available, and otherwise still states
  the release and failing file.
- **SC-007**: An administrator completes "select a range, generate the bundle, copy the text" in three
  actions and under 60 seconds.
- **SC-008**: Enabling capture and the forwarding window changes nothing observable in normal operation
  — no page-load regression beyond 5%, no failed user action attributable to telemetry in a 24-hour
  soak with the store made unavailable for part of it.

## Assumptions

- The administrator of an instance is the person who reads issues, generates bundles and opens the
  forwarding window; there is no end-user consent flow because the only person who can send data is the
  person who administers the instance.
- The remote tracker (for example Rustrak) is deployed and operated outside this application; this
  feature configures it and sends to it, and never ships or manages it.
- Local capture is enabled by default and can be turned off; forwarding is disabled by default and
  requires both a configured endpoint and an open window.
- The project is open source, so the source behind a release is public; source maps are still not
  served publicly and are not bundled with reports unless explicitly requested.
- Archive titles and filenames are treated as sensitive and excluded by default; an explicit opt-in is
  required to include them, and share codes/tokens are always removed.
- The defaults (retention 7 days, 50,000 events, 512 MB store, 64 KB per event, 20 MB per bundle) are
  reasonable for a self-hosted single-administrator deployment; every one of them is administrator
  adjustable, and a storage quota is offered because bytes are easier to reason about than a count.
- The feedback text is produced in both Chinese and English because the maintainer and the reporting
  administrator may not share a language; the user picks which to paste.
- A Sentry-protocol-compatible tracker is assumed to accept the standard envelope and to need no
  integration beyond its address and project key; if a tracker deviates, that is its compatibility
  gap, not a change to this feature's scope.
- Bundles are delivered to the maintainer by the administrator (attached to an issue or carried
  manually); the application has no channel to deliver them by itself.
