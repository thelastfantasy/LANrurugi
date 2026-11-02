# Feature Specification: Restricted Guest Access Mode

**Feature Branch**: `007-guest-restricted-access`

**Created**: 2026-08-27

**Status**: Draft

**Input**: User description: "受限访客模式：未登录访问时，若管理员已启用访客模式总开关且至少一个分类被标记为对访客可见，则以新的 guest_restricted 角色浏览受限内容，而不是强制跳转登录页；否则保持现状强制登录。管理员密码登录恒定开启（移除 enablepass/nofunmode 两个可关闭开关），访客模式是「总开关 + 按分类授权」两层结构，访客能力边界为浏览+阅读+分类范围内搜索/标签筛选，不能收藏/保存进度/下载原始文件/访问任何管理功能。devmode 一并从设置页移除，改用环境变量控制。"

## User Scenarios & Testing *(mandatory)*

<!--
  IMPORTANT: User stories should be PRIORITIZED as user journeys ordered by importance.
  Each user story/journey must be INDEPENDENTLY TESTABLE - meaning if you implement just ONE of them,
  you should still have a viable MVP (Minimum Viable Product) that delivers value.

  Assign priorities (P1, P2, P3, etc.) to each story, where P1 is the most critical.
  Think of each story as a standalone slice of functionality that can be:
  - Developed independently
  - Tested independently
  - Deployed independently
  - Demonstrated to users independently
-->

### User Story 1 - Administrator enforces mandatory password login (Priority: P1)

An administrator deploying or already running LANrurugi can no longer configure the library to be
fully open (no password) or to skip login for any request. Password-based login is always
required to reach any administrative or management function.

**Why this priority**: This is the foundational trust-model change everything else in this
feature depends on — without it, "restricted guest access" would just be one more configurable
option layered on top of an already-optional password wall, not a real security boundary.

**Independent Test**: Deploy (or use an existing) instance, confirm there is no setting anywhere
that disables the password requirement, and confirm every request to a management/administrative
endpoint (settings, plugins, backup, user/token management, etc.) is rejected unless the caller
has a valid administrator session.

**Acceptance Scenarios**:

1. **Given** a fresh LANrurugi deployment, **When** the administrator opens the Settings page,
   **Then** there is no "enable password" or "No-Fun mode" toggle — password-based login is
   simply how the system always behaves, with no way to turn it off.
2. **Given** an instance previously configured with password protection disabled (a pre-upgrade
   library), **When** the instance is upgraded to include this feature, **Then** the
   administrator must complete a real password login to reach any management function — the
   library is no longer reachable as a fully open, no-login system.
3. **Given** an unauthenticated caller, **When** they attempt to reach any settings, plugin
   management, backup, or account/token management function, **Then** the request is rejected
   regardless of any prior configuration.

---

### User Story 2 - Administrator opens specific categories to unauthenticated visitors (Priority: P1)

An administrator who wants to let people browse and read part of their library without requiring
a login (for example, sharing a curated, publicly-appropriate subset of their collection) can turn
on a single "guest mode" switch and then mark specific existing categories as visible to guests.
Visitors who are not logged in then land directly on a browsable library scoped to only those
categories, instead of being forced to a login screen.

**Why this priority**: This is the actual value proposition of the feature — the reason the
strict all-or-nothing password wall (User Story 1) is being introduced alongside a way to
selectively re-open part of the library, rather than simply making the whole system stricter.

**Independent Test**: With guest mode enabled and at least one category marked guest-visible, open
the site in a fresh, unauthenticated browser session and confirm the library view shows only
archives belonging to guest-visible categories, with a visible way to log in as an administrator
instead.

**Acceptance Scenarios**:

1. **Given** guest mode is enabled and category "Public Picks" is marked visible to guests,
   **When** an unauthenticated visitor opens the site, **Then** they see a library view containing
   only the archives in "Public Picks" (and any other guest-visible categories), with no forced
   redirect to a login page.
2. **Given** the same state as above, **When** the visitor clicks a login entry point in the site
   navigation, **Then** they are taken to the existing administrator password login flow.
3. **Given** guest mode is enabled but no category has been marked visible to guests yet, **When**
   an unauthenticated visitor opens the site, **Then** they are redirected to the login page (the
   same behavior as if guest mode were off) — an enabled-but-unconfigured guest mode never
   presents an empty or confusing "nothing here" library view in place of the login page.
4. **Given** guest mode is turned off entirely, **When** an unauthenticated visitor opens the
   site, **Then** they are redirected to the login page, exactly as before this feature existed.
5. **Given** an instance upgraded from a version predating this feature, **When** the upgrade
   completes, **Then** guest mode is off and no category is marked guest-visible by default — the
   administrator must explicitly opt in to any unauthenticated browsing.

---

### User Story 3 - Guest visitor browses, reads, and searches within their authorized scope (Priority: P2)

Within the categories an administrator has opened up, an unauthenticated guest visitor can browse
archive listings, open and read an archive in the reader, and search or filter by tag — but only
ever within that authorized scope, and only for those specific capabilities.

**Why this priority**: This defines the actual guest experience once User Story 2's access has
been granted — without it, "guest mode" would only be a landing-page routing change with no real
browsing capability behind it. Ranked below User Story 2 because the routing/gating behavior is
the prerequisite; this story is about what happens once a guest is already in.

**Independent Test**: As an unauthenticated guest scoped to a known set of categories, verify
browsing, reading, and search/tag-filter all work and stay within scope, and verify an attempt to
reach an archive outside that scope (by guessing/constructing a direct link) fails the same way a
nonexistent archive would.

**Acceptance Scenarios**:

1. **Given** a guest-visible category containing several archives, **When** the guest opens the
   library listing, **Then** they see those archives and can open one to read it in the reader
   (viewing pages).
2. **Given** the same guest-visible scope, **When** the guest searches by keyword or filters by
   tag, **Then** results are limited to archives within guest-visible categories only — no archive
   outside that scope appears in results, tag/autocomplete suggestions, or any other listing.
3. **Given** an archive that exists in the library but does not belong to any guest-visible
   category, **When** a guest attempts to open it directly (e.g. via a guessed or previously seen
   URL), **Then** access is denied in a way that reveals no information about whether the archive
   exists.
4. **Given** a guest is viewing an archive, **When** they attempt to bookmark it, save reading
   progress, or download the original file, **Then** the action is unavailable to them.
5. **Given** a guest visitor, **When** they attempt to reach any settings, plugin management,
   activity log, statistics, or other administrative page, **Then** access is denied.

---

### Edge Cases

- What happens if an administrator marks a category guest-visible, and that category is later
  deleted or unpinned? Guest access to any archives that were only reachable through that category
  must be revoked as part of the same change — there must be no window where guests retain access
  through stale category membership.
- What happens if an archive belongs to both a guest-visible category and a non-guest-visible
  category? It remains visible to guests (visibility is a union across all categories an archive
  belongs to, not an intersection) — being in at least one open category is enough.
- What happens when a guest's browsing session is active and the administrator turns guest mode
  off, or un-marks the last guest-visible category, mid-session? The next request the guest makes
  must be treated under the new state (redirected to login / denied) — guest access must not
  persist based on a stale snapshot of configuration from earlier in the session.
- What happens if a third-party client (e.g. a Tachiyomi/Mihon-style extension) that expects the
  legacy `has_password`/`nofun_mode` API fields queries this instance? See FR-014 for the required
  compatibility behavior.
- What happens if an administrator enables guest mode but marks zero categories, saves, and later
  reopens the settings page? The system must clearly communicate that guest mode is "on" but
  currently has no effect (falls back to forced login) rather than implying guests can already
  browse.

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: The system MUST require a valid administrator password login to access any
  settings, plugin management, backup/restore, account/token management, activity log, or other
  administrative function. There MUST be no configuration that disables this requirement.
- **FR-002**: The system MUST NOT provide any setting that allows the entire library to be made
  readable or writable by unauthenticated callers without restriction (the prior "fully open, no
  password" mode is removed entirely, not merely defaulted off).
- **FR-003**: The system MUST provide a single, top-level "guest mode" setting that an
  administrator can turn on or off, independent of which categories are marked guest-visible.
- **FR-004**: The system MUST allow an administrator to mark any existing category as visible to
  guests, and to unmark it, from the same place categories are otherwise managed.
- **FR-005**: When guest mode is on AND at least one category is marked guest-visible, an
  unauthenticated visitor MUST be able to browse the library without being redirected to a login
  page, scoped to archives belonging to at least one guest-visible category.
- **FR-006**: When guest mode is off, OR guest mode is on but zero categories are marked
  guest-visible, an unauthenticated visitor MUST be redirected to the login page — identical to
  the system's behavior before this feature existed.
- **FR-007**: The system MUST provide a visible, always-available way for an unauthenticated
  guest visitor to reach the administrator login flow (e.g. a persistent navigation entry point),
  regardless of whether guest mode is currently granting them any browsing access.
- **FR-008**: An unauthenticated guest visitor operating within their authorized scope MUST be
  able to: view archive listings, open and read archive contents (view pages), and search/filter
  by tag or keyword, with all such results limited to archives in guest-visible categories.
- **FR-009**: An unauthenticated guest visitor MUST NOT be able to: bookmark an archive, save
  reading progress (locally or server-side), download an original archive file, or access any
  archive that does not belong to at least one guest-visible category.
- **FR-010**: An unauthenticated guest visitor MUST NOT be able to access any administrative
  function listed in FR-001, regardless of guest mode state.
- **FR-011**: A guest visitor's attempt to access an archive outside their authorized scope MUST
  be denied without revealing whether that archive exists (the response for "exists but
  unauthorized" and "does not exist" must be indistinguishable to the caller).
- **FR-012**: Search, tag-filter, tag-cloud, and any autocomplete/suggestion feature available to
  a guest visitor MUST NOT surface information (titles, tags, counts, or existence) about archives
  outside guest-visible categories.
- **FR-013**: On an instance upgraded from a version predating this feature, guest mode MUST
  default to off and no category MUST default to guest-visible — the administrator must take an
  explicit action to enable any unauthenticated browsing. An instance previously configured with
  password protection disabled MUST require a real administrator login after the upgrade (see
  Edge Cases and User Story 1, Scenario 2).
- **FR-014**: Any existing external API surface that previously reported whether password
  protection was enabled MUST continue to report a value third-party clients can parse without
  error after this feature ships, reflecting that password protection is now always effectively
  enabled — this is a compatibility requirement for existing third-party integrations, not a
  reintroduction of the removed toggle.
- **FR-015**: The system MUST re-evaluate a guest visitor's access on every request against the
  current guest-mode/category-visibility configuration — a change to that configuration MUST take
  effect for guest visitors immediately, not only for new sessions started after the change.
- **FR-016**: If a category is deleted, unpinned, or has its guest-visible marking removed, any
  archive that was only reachable through that category MUST no longer be accessible to guest
  visitors as of the same change (see Edge Cases).
- **FR-017**: The debug-mode setting (previously exposed on the Settings page, controlling only
  whether the client skips checking for software updates) MUST be removed from the Settings page.
  The underlying behavior it controlled MUST remain available through a means the administrator
  configures outside of the running application's own user-facing settings (e.g. at deployment
  time), so that suppressing the update check remains possible without requiring it to be a
  runtime-toggleable, guest/security-adjacent setting.

### Key Entities

- **Category**: An existing grouping of archives, administrator-defined. Gains a new attribute
  indicating whether it is visible to unauthenticated guest visitors. An archive belongs to zero
  or more categories; guest visibility of an archive is determined by whether it belongs to at
  least one guest-visible category.
- **Guest Mode Setting**: A single instance-wide flag, independent of any individual category,
  that must be on for guest-visible categories to actually grant unauthenticated access. Acts as a
  master switch layered on top of per-category visibility.
- **Guest Visitor**: An unauthenticated caller accessing the system while guest mode is active and
  at least one category is guest-visible. Distinct from an administrator (always requires
  password login) and from any existing API-token-based access method — a guest visitor's access
  is scoped to guest-visible categories and limited to browsing/reading/searching, with no write
  capabilities.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: An administrator can go from "guest mode off" to "unauthenticated visitors can
  browse a chosen subset of the library" in under 2 minutes, using only the guest-mode switch and
  existing category management, without needing to configure anything per-archive.
- **SC-002**: 100% of archives outside an administrator's chosen guest-visible categories are
  unreachable by an unauthenticated visitor, verified by direct-access attempts, search, and
  browsing — zero information leakage (title, tag, existence) about out-of-scope archives.
- **SC-003**: 100% of administrative functions (settings, plugin management, backup, account/token
  management, activity/statistics) reject unauthenticated access, with no configuration path that
  changes this.
- **SC-004**: Every instance upgraded from a pre-feature version preserves administrator access
  (no one is locked out of their own library) while defaulting to the strictest guest posture
  (no unauthenticated browsing) until the administrator explicitly opts in.
- **SC-005**: A guest visitor can complete "land on the site → browse a guest-visible category →
  open and read an archive" without encountering a login prompt, and can reach the login flow via
  a single, discoverable action at any point in that journey.

## Assumptions

- "Categories" in this feature refers to the existing category grouping mechanism already present
  in the library; no new grouping concept is introduced for scoping guest access.
- A guest visitor is not associated with any persistent identity across visits (no guest accounts,
  no guest-specific saved state) — every unauthenticated visit is evaluated fresh against current
  guest-mode/category configuration.
- The existing API-token-based access system (used by third-party clients/automation) is
  unaffected by this feature except for the compatibility field described in FR-014; API tokens
  are not a mechanism for granting or scoping guest access.
- "No-Fun mode" (a legacy setting that forced login even when password protection was otherwise
  disabled) has no remaining purpose once password protection can no longer be disabled, and is
  removed as part of this feature rather than being preserved as a separate toggle.
- Suppressing the software-update check (previously "debug mode") is a deployment-time/operational
  concern, not a security or content-access concern, and is intentionally addressed separately
  from — and does not block — the guest-access functionality this spec defines.
