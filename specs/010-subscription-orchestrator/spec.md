# Feature Specification: Subscription Orchestrator

**Feature Branch**: `010-subscription-orchestrator`

**Created**: 2026-10-01

**Status**: Draft

**Input**: User description: "新增订阅类插件功能（issue #55）。本身无下载功能，依赖于下载插件。订阅固定 uploader、评价分数、订阅 tag、排除 tag 和分类、定期执行、失败 fallback、开关 metadata 插件、自动执行下载、积分限制、超出预算条件、超出预算后的行为、自定义设置页面、分类设置；在 /upload 加一个『预约』分组，把匹配预约条件但无法下载的 source URL 和理由存进去（默认折叠，留一个 trigger 图标按钮）。"

**Framing decision (from the requester)**: this is **not** a fifth plugin type. It is an
*orchestrator* — a component that drives the existing metadata/download plugins on a schedule,
in the spirit of qBittorrent's RSS component. Users compose rules; the orchestrator matches
candidates against those rules and hands the survivors to the existing download queue.

## Clarifications

### Session 2026-10-02

- Q: 订阅去检查某个创作者或 tag 的新作品时，候选作品列表从哪里来？ → A: 插件新增一个可选的「搜索/发现」能力：给定条件返回候选 source URL 列表；宿主负责调度、过滤、去重
- Q: 判断一个候选作品「已经在库里、不用再下」时，依据什么来比对？ → A: 下载前比 source URL（归一化后），下载后再比内容哈希；并复用版本链判断，使同作品的新版本不被当成全新作品
- Q: 订阅的创建和管理界面应该放在哪里？ → A: 订阅管理放设置页（新建「订阅」分区），预约列表留在上传页
- Q: 服务停机期间错过的检查周期，重启后应该怎么处理？ → A: 错过多少轮都只补跑一次，然后回到正常周期
- Q: 订阅匹配到的作品，应该直接开始下载，还是先进队列等用户确认？ → A: 每个订阅可选，默认为「需确认」，用户确信规则可靠后自行开启自动下载
- Q: 「一览来源」（RSS 或 HTML 列表页）应该由谁来解析？ → A: 一个发现接口支持两种输入——插件自建来源（按条件搜索），或用户提供 URL（插件从该 feed/页面抽取链接）
- Q: 需要登录才能看到的订阅源，登录态怎么获得？ → A: 优先沿用现有登录插件机制（扩展声明依赖的登录插件，宿主在每次发现调用前注入登录态），不另建一套
- Q: 新增「发现」这种扩展能力后，AI 插件生成向导要不要也能生成它？ → A: 要——向导必须覆盖发现能力，否则新站点仍须手写，与向导存在的目的相悖
- Q: 发现频率和登录选项，分别放在插件层还是订阅层？ → A: 频率走三级合并（扩展声明建议值与下限，订阅在其之上自选）；登录维持扩展层单份凭据，订阅只声明依赖哪个登录

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Track a creator and get new works automatically (Priority: P1)

A user follows a particular uploader/creator. They create a subscription naming that uploader,
pick how often it should check, and choose a category for anything it finds. From then on the
system periodically looks for that uploader's new works and brings them in without the user having
to hunt for them. New arrivals show up in the library like any other download. By default the user
confirms each batch before it downloads; once they trust the rules, they can let that subscription
download on its own.

**Why this priority**: This is the feature's core promise and the smallest thing that is useful on
its own. Everything else (filters, budgets, the reservation list) refines this loop rather than
replacing it. Shipping only this already removes the manual "check the site, paste URLs" chore.

**Independent Test**: Create one subscription for a known uploader with a short interval, wait for
one cycle, and confirm matching works appear in the download queue and then the library — with no
manual URL entry at any point.

**Acceptance Scenarios**:

1. **Given** a subscription for uploader "X" with no filters and automatic download enabled,
   **When** a check cycle runs and the source has two works by X that are not in the library,
   **Then** both are queued for download and attributed to that subscription.
1a. **Given** the same subscription left at the default (wait for confirmation), **When** the same
   cycle runs, **Then** both works are presented for approval and neither consumes download credit
   until the user approves them.
2. **Given** the same subscription, **When** a later cycle runs and finds the same two works again,
   **Then** neither is queued a second time.
3. **Given** a subscription with a target category, **When** one of its downloads completes,
   **Then** the resulting archive belongs to that category.
4. **Given** a subscription the user has disabled, **When** its scheduled time arrives,
   **Then** no check runs and nothing is queued.

---

### User Story 2 - Narrow what gets downloaded (Priority: P2)

The user refines a subscription so it stops pulling in things they do not want: require certain
tags, exclude others, require a minimum community rating, and ignore works already belonging to
chosen categories. They can see which rule rejected a given candidate, so a rule that is too strict
is diagnosable rather than mysteriously silent.

**Why this priority**: Without filters an uploader subscription becomes a firehose and users turn
it off. But it is only valuable once P1 works, so it follows rather than leads.

**Independent Test**: Add an exclude-tag rule to an existing subscription, run a cycle against a
candidate set containing that tag, and confirm the item is not queued and is recorded as rejected
with the rule that rejected it.

**Acceptance Scenarios**:

1. **Given** a subscription requiring tag "A" and excluding tag "B", **When** a candidate has both,
   **Then** it is not queued and the exclusion is recorded as the reason.
2. **Given** a subscription with a minimum rating, **When** a candidate is below it,
   **Then** it is not queued; **When** a later cycle sees the same work now rated above the
   threshold, **Then** it is queued.
3. **Given** a subscription excluding a category, **When** a candidate already belongs to that
   category in the library, **Then** it is not queued.

---

### User Story 3 - See and act on what could not be downloaded (Priority: P2)

Some matches cannot be downloaded when they are found — the source is unreachable, the user is out
of download credit, or the download failed for another reason. Instead of being lost or silently
retried forever, these land in a **reservation list** on the upload page: each entry shows the
source URL and why it did not go through. The user can retry one, retry all, or discard. The list
is collapsed by default behind an icon button so it never gets in the way when empty.

**Why this priority**: Equal in importance to filtering: without it, failures are invisible and the
user cannot tell "nothing matched" from "everything failed". The requester called this out
explicitly as part of the feature.

**Independent Test**: Force a failure (point a subscription at an unreachable source), run a cycle,
and confirm an entry appears in the reservation list with the real reason — and that retrying it
after restoring the source completes the download.

**Acceptance Scenarios**:

1. **Given** a candidate that passed all rules, **When** the download cannot start because the
   source is unreachable, **Then** a reservation entry is created carrying the source URL and that
   reason, and no partial archive is left behind.
2. **Given** a reservation entry, **When** the user retries it and the obstacle is gone,
   **Then** it downloads normally and leaves the reservation list.
3. **Given** an empty reservation list, **When** the user opens the upload page,
   **Then** the group is collapsed and does not occupy meaningful space.
4. **Given** a reservation entry the user discards, **When** a later cycle sees the same work,
   **Then** it is not re-reserved (the discard is remembered).

---

### User Story 4 - Stay within a download budget (Priority: P3)

Downloading can consume a finite per-user resource at the source (for example E-Hentai's GP). The
user sets how the subscription should behave when a download fails for lack of that resource:
pause the subscription, or keep going and simply reserve the ones that failed. The system reports
which subscriptions are paused for this reason and lets the user resume them.

**Why this priority**: Only matters once subscriptions run unattended at volume, and it is the part
most constrained by what sources actually expose (see Assumptions). Deferring it does not block
P1-P3.

**Independent Test**: Configure a subscription to pause on insufficient credit, simulate that
failure, and confirm the subscription stops scheduling further checks and is shown as paused with
that reason.

**Acceptance Scenarios**:

1. **Given** a subscription set to pause on insufficient credit, **When** a download fails for that
   reason, **Then** the subscription stops running and is shown as paused with the reason.
2. **Given** a subscription set to continue, **When** the same failure occurs,
   **Then** the item is reserved and the subscription keeps checking on schedule.
3. **Given** a paused subscription, **When** the user resumes it,
   **Then** it resumes on its normal schedule.

---

### Edge Cases

- A check cycle is still running when the next one is due — cycles for the same subscription must
  not overlap or double-queue.
- The source returns nothing, or returns an error, for an entire cycle: this is not a failure of the
  subscription and must not disable it.
- Two subscriptions match the same work in one cycle — it must be queued once, not twice.
- A work matches a subscription but is already in the library (by content or by source URL) — it
  must not be re-downloaded.
- A work matches a subscription and is a newer revision of something already held — it must be
  recognised as an update, not skipped as a duplicate nor queued as an unrelated new work.
- A subscription references a category, a plugin, or a source that no longer exists.
- A source raises its declared minimum interval after subscriptions were already created at a shorter
  one — existing subscriptions must be brought into line and the user told, not left violating it.
- A user-supplied listing URL stops resolving, changes shape, or starts returning an error page
  instead of a listing — the subscription must surface this rather than silently finding nothing.
- A source's sign-in expires between checks, so a listing that worked yesterday returns a login page
  today — this must be reported as an authentication problem, not as an empty listing.
- A source returns a *valid but smaller* listing when signed out (rather than an error), so the check
  appears to succeed while missing works that are only visible to a signed-in account.
- A listing contains entries that are not works at all (ads, pagination links, site notices).
- The user edits a subscription's rules while one of its cycles is mid-flight.
- The server restarts between "candidate matched" and "download queued".
- A reservation entry's source URL no longer resolves when retried much later.
- The reservation list grows without bound because nobody ever clears it.
- A subscription's interval is set so short that cycles cannot keep up.

## Requirements *(mandatory)*

### Functional Requirements

**Subscription definition**

- **FR-001**: Users MUST be able to create, edit, enable/disable, and delete subscriptions.
- **FR-002**: A subscription MUST identify what it tracks (at minimum: a specific uploader/creator,
  and/or a set of tags) against a source the system already knows how to read.
- **FR-002a**: Candidate discovery MUST be provided by the source-specific extension, which — given
  a subscription's criteria — returns the list of candidate works it found. The orchestrator MUST NOT
  contain per-source search logic of its own; adding support for a new source MUST NOT require
  changing the orchestrator.
- **FR-002c**: Discovery MUST accept two kinds of input through the same interface, because both
  produce the same thing — a list of candidate works:
  - **Extension-supplied listing**: the extension knows how to search its own site from the
    subscription's criteria (e.g. "everything by this uploader"). Requires no URL from the user.
  - **User-supplied listing URL**: the user points the subscription at a feed or listing page, and
    the extension extracts candidate works from it. This covers sources with no dedicated extension
    search, and any site that publishes a feed.
- **FR-002d**: A listing MUST be re-read on each check so that newly published works are picked up;
  works already seen in an earlier check MUST NOT be re-proposed (see FR-009a).
- **FR-002e**: Where a source requires signing in before its listing is visible, discovery MUST reuse
  the application's existing login mechanism — the same one metadata and download already rely on —
  rather than introducing separate credentials for subscriptions. An extension declares which login
  it depends on, and the system supplies the signed-in state on each discovery call.
- **FR-002e1**: Sign-in credentials remain held per source extension, not per subscription. Several
  subscriptions against the same source therefore share one signed-in identity, and a subscription
  only records *which* login it relies on. Per-subscription credentials are deliberately out of scope:
  they would mean a second place to store a secret for no benefit this feature needs.
- **FR-002f**: When a check fails because the signed-in state is missing, expired, or rejected, the
  system MUST report that specific cause, and MUST NOT silently report "nothing found" — which is
  indistinguishable from a working subscription with no new works.
- **FR-002g**: A subscription MUST be checked under the same signed-in identity the user expects it
  to use. Sources commonly return a *different candidate set* depending on who is asking — content
  visible only to signed-in accounts, per-account content filters, or an entirely separate
  signed-in-only catalogue. A check that silently runs signed-out would therefore not fail; it would
  succeed against a smaller world and quietly miss works the user was subscribing for.
- **FR-002h**: Because of FR-002g, a check that runs without the expected signed-in state MUST be
  treated as inconclusive rather than as a completed check: the system MUST NOT record its candidates
  as "everything that exists", MUST NOT let it mark works as seen-and-dismissed, and MUST surface the
  degraded state to the user.
- **FR-002b**: A source whose extension offers no discovery capability MUST NOT be selectable when
  creating a subscription, and the reason MUST be visible to the user rather than failing silently
  at the first check.
- **FR-003**: A subscription MUST let the user set required tags, excluded tags, a minimum rating,
  and categories to ignore.
- **FR-004**: A subscription MUST let the user choose how often it runs, and that choice MUST be
  per-subscription rather than one global interval.
- **FR-004a**: A source extension MUST be able to declare both a *suggested* check interval and a
  *minimum* one. The two layers hold different knowledge and neither alone is sufficient: the
  extension knows what its site tolerates (rate limits, ban risk), while the user knows how closely
  they want to follow a given creator or tag.
- **FR-004b**: A subscription MUST NOT be allowed to check more often than its source's declared
  minimum. An attempt to do so MUST be refused with the reason shown, rather than silently clamped —
  a user who thinks they set 5 minutes but is actually getting 60 would misread every later result.
- **FR-004c**: Where a source declares no interval guidance, the subscription's own choice applies
  unchanged.
- **FR-005**: A subscription MUST let the user choose a target category for what it downloads.
- **FR-006**: A subscription MUST let the user control whether metadata enrichment runs for its
  downloads.
- **FR-007**: The system MUST let the user run a subscription immediately, without waiting for its
  schedule, and see the result of that run.
- **FR-007a**: Each subscription MUST let the user choose whether matched works download
  automatically or wait for explicit confirmation. The default MUST be *wait for confirmation*: a
  newly created subscription's rules are usually still being tuned, which is exactly when an
  over-broad rule would otherwise spend real download credit unattended.
- **FR-007b**: When a subscription is set to wait for confirmation, matched works MUST be presented
  for the user to approve or dismiss, individually or together, and MUST NOT consume download credit
  until approved.

**Matching and queueing**

- **FR-008**: The system MUST periodically check each enabled subscription according to its own
  interval.
- **FR-008a**: When a subscription's scheduled checks were missed (the service was not running), the
  system MUST run exactly one catch-up check and then resume the normal interval — regardless of how
  many intervals elapsed. Checks are idempotent, so replaying each missed interval would see the same
  candidates while multiplying load and credit use at startup.
- **FR-009**: The system MUST NOT queue a work that is already in the library, or that it has
  already queued for the same subscription.
- **FR-009a**: Before downloading, the system MUST recognise an already-held work by its source
  identity (normalised so that the same work referred to by different URL forms is recognised as
  one). After downloading, it MUST additionally recognise duplicates by content, so a work that
  slipped past the pre-download check is not catalogued twice.
- **FR-009b**: A work that is a *newer revision* of something already held MUST NOT be treated as a
  duplicate and skipped. It MUST be handled by the existing revision-comparison behaviour, so that a
  subscription can pick up updated versions rather than ignoring them forever.
- **FR-010**: The system MUST queue at most one download per distinct work per cycle, even when
  several subscriptions match it.
- **FR-011**: The system MUST record, per candidate it rejected, which rule rejected it, and expose
  that to the user.
- **FR-012**: The system MUST NOT run overlapping cycles for the same subscription.
- **FR-013**: The system MUST survive restart without re-downloading works it had already handled,
  and without losing subscriptions or their state.

**Reservation list**

- **FR-014**: When a matched work cannot be downloaded, the system MUST record a reservation entry
  carrying the source URL and the reason.
- **FR-015**: Users MUST be able to retry a reservation entry individually, retry all, or discard
  entries.
- **FR-016**: A discarded entry MUST NOT be re-created by later cycles for the same work.
- **FR-017**: The reservation list MUST appear on the upload page as a group that is collapsed by
  default and reachable via an icon control.
- **FR-018**: The system MUST bound the reservation list's growth (age or count), and MUST make
  clear to the user when entries were dropped for that reason.

**Budget behavior**

- **FR-019**: Users MUST be able to choose what a subscription does when a download fails for lack
  of source-side credit: pause the subscription, or continue and reserve the failures.
- **FR-020**: The system MUST show which subscriptions are paused and why, and MUST let the user
  resume them.

**Visibility**

- **FR-021**: Users MUST be able to see, per subscription: when it last ran, what it found, what it
  queued, and what it rejected or reserved.
- **FR-022**: The system MUST record subscription-initiated downloads distinguishably from
  user-initiated ones, so an unattended download's origin is auditable.

**Authoring new sources**

- **FR-027**: The assisted plugin-authoring flow MUST be able to produce the discovery capability,
  not only the existing metadata/login/download ones. Discovery is the capability a new subscription
  source most needs, so leaving it out would mean every new source still has to be written by hand —
  defeating the purpose of having an authoring assistant at all.
- **FR-028**: The authoring flow's notion of "which capabilities exist" MUST be derived from one
  place, so that adding a capability does not require finding and updating several hardcoded lists.
  *(Verified during clarification: the current flow hardcodes its capability list in more than one
  location, so this is a real change rather than a restatement of existing behaviour.)*
- **FR-029**: A generated discovery capability MUST be verifiable before it is saved, in the same way
  other generated capabilities already are — the author should see whether it actually returns
  candidates from a real listing, rather than discovering it is broken at the first scheduled check.

**Configuration surface**

- **FR-023**: Subscription settings MUST be editable through the application's own interface, and
  MUST NOT require the user to author or trust markup supplied by a plugin.
  *(See Assumptions — this deliberately narrows the original "自定义设置页面（自带 HTML）" wording.)*
- **FR-024**: Subscription management (create / edit / enable / disable / delete / run-now) MUST live
  in the application's settings area, alongside the other long-lived configuration sections —
  subscriptions are standing configuration, not a one-off action.
- **FR-025**: The reservation list MUST remain on the upload page (FR-017), separate from
  subscription management. It is a queue of items awaiting the user's decision, so it belongs beside
  the download queue the user is already working in, not inside a settings screen.
- **FR-026**: From the reservation list, the user MUST be able to reach the subscription that
  produced an entry, so that a rule which keeps generating failures can be found and corrected
  without hunting for it.

### Key Entities

- **Subscription**: A user-defined standing instruction. Carries what it tracks, its filter rules,
  its schedule, its target category, its budget behavior, whether it is enabled or paused (and why),
  and when it last ran.
- **Check Cycle**: One execution of one subscription. Carries when it ran, how many candidates it
  saw, and what became of each (queued / rejected-with-reason / reserved / already-present).
- **Candidate**: One work a cycle considered, identified by its source URL, with the outcome and
  (when rejected) the rule responsible.
- **Reservation Entry**: A matched work that could not be downloaded. Carries the source URL, the
  reason, which subscription produced it, when, and whether the user has discarded it.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: A user can create a working subscription in under 2 minutes without consulting
  documentation.
- **SC-002**: Once a subscription is created *and the user has enabled automatic download for it*,
  new matching works reach the library with no further interaction. Until then, matched works wait
  for approval rather than downloading unattended.
- **SC-003**: Across repeated cycles, the same work is never downloaded twice.
- **SC-004**: For every candidate a cycle rejected, the user can see which rule rejected it.
- **SC-005**: Every matched work that failed to download is either visible in the reservation list
  or was explicitly discarded by the user — none are silently lost.
- **SC-006**: Subscriptions running in the background do not noticeably degrade interactive use of
  the application.
- **SC-007**: After a restart, subscriptions resume on schedule with no duplicate downloads and no
  lost reservation entries.
- **SC-008**: When the reservation list is empty, it costs the user no visible space or attention on
  the upload page.

## Assumptions

- **Source-side credit cannot be queried in advance.** Verified against this repository: the only
  signal available is the failure text a download returns (E-Hentai's "Insufficient funds"); there is
  no balance to read beforehand. Budget behavior is therefore **reactive** — the system responds to a
  failure rather than predicting one. The original wording "超出预算条件（单项 GP 不够 or 整体积分
  不够）" is narrowed accordingly, as confirmed with the requester.
- **No plugin-supplied HTML settings page.** The original description asked for "自定义设置页面
  （自带 HTML）". Rendering markup authored by a plugin is at odds with the project's plugin
  sandboxing posture (constitution Principle IV), which exists so a plugin cannot reach outside its
  sandbox — markup injected into the application's own page does exactly that. Subscription settings
  are therefore rendered by the application from declared fields. If per-source custom fields are
  needed later, they should be *declared data* the application renders, never markup.
- **Markup parsing already exists; no new parser is needed.** Verified against this repository: the
  plugin runtime already ships an HTML/XML parser with a CSS-subset selector API, including an XML
  mode for case-sensitive documents. RSS and Atom are XML, so an extension can read a feed or a
  listing page with what it already has. This feature therefore adds a *discovery* capability, not a
  parsing one — an important distinction, since a separate "parser plugin" interface was considered
  and found unnecessary.

- **Sign-in reuses the existing login mechanism.** Verified against this repository: extensions
  already declare which login they depend on, and the host injects the resulting signed-in state into
  every call. Discovery rides the same path, so a source the user can already download from is one
  they can already subscribe to — no second place to enter credentials, and no new secret storage.
- **Signed-in state changes *what exists*, not just *what is permitted*.** On real sources (E-Hentai
  being the worked example the requester raised), searching while signed in returns a different result
  set than searching signed out — not an error, simply a different world. Subscriptions are therefore
  specified to treat a missing sign-in as an inconclusive check (FR-002g/h), because the failure mode
  here is silent under-reporting, which no error message would reveal.

- **The authoring assistant needs widening, not just reuse.** Verified during clarification: the
  assisted plugin-authoring flow enumerates the capabilities it can produce as hardcoded literals in
  several places rather than reading one shared definition. Discovery therefore will not appear there
  automatically, and FR-028 treats consolidating that list as part of this feature rather than
  assuming it already works.

- **Interval guidance follows the existing two-layer settings pattern.** The application already has
  a shape for "the extension declares a default, the user may override it" and uses it for other
  download behaviour. Check intervals reuse that shape rather than inventing a parallel one, with the
  addition of a floor the user cannot go under (FR-004b) because the risk being guarded against —
  getting the user rate-limited or banned — is not the user's to discover by trial.
- **Credentials stay per source, not per subscription.** Verified against this repository: login
  values are stored once per extension namespace, so multiple subscriptions on one source already
  share an identity. Keeping it that way avoids introducing a second credential store and a second
  secret-handling surface for a capability ("several accounts on one site") this feature has no need
  of.

- **Subscriptions reuse the existing download pipeline.** They queue work through the same path as a
  manual download, inheriting its concurrency limits, rate limiting, duplicate handling, and version
  comparison rather than re-implementing any of it.
- **Subscriptions are an orchestration layer, not a new plugin kind.** Per the requester's framing,
  the feature drives existing metadata/download plugins; it does not introduce a fifth plugin type.
- **Scheduling is best-effort, not real-time.** A cycle may run late (server asleep, previous cycle
  still running). Users expect "about every N hours", not a guarantee. Downtime collapses into a
  single catch-up check rather than a backlog (FR-008a).
- **Single-user deployment.** Subscriptions are not scoped per-account; this matches how the rest of
  the application treats user state today.
- **Which sources are supported is bounded by existing plugins.** Subscriptions can only track
  sources the system can already read and download; this feature does not add source support.

## Out of Scope

- Adding support for new sources or sites.
- Predicting or displaying source-side credit balances.
- Plugin-authored markup rendered inside the application.
- Per-account subscription ownership or sharing.
- Notifying the user outside the application (email, push, webhooks).
