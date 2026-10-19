# Feature Specification: Agent-Harness Hardening for LANrurugi's AI Call Sites

**Feature Branch**: `014-ai-harness-hardening`

**Created**: 2026-10-16

**Status**: Draft

**Input**: User description: "归纳一下 lanrurugi 中的各处 ai 调用，并汇总出来，然后分析哪些地方有必要用 harness 的概念进行工程强化（暂时不用改代码进行实装）。" Follow-up: "就是智能体harness的定义" — i.e. the reference is the *agent harness* (the non-model scaffolding that turns a one-shot completion into a bounded, observable, steerable, reproducible autonomous process), not generic service engineering. Then: "你可以用 speckit 先做，然后在 implement 前面停下来就行了".

## Context

### What exists today

LANrurugi has six distinct families of AI call, not one:

| Family | Count | Notes |
| --- | --- | --- |
| Server-side LLM calls | **14 call sites / 9 features** | All through one client abstraction; single provider |
| Cloud translation | **1 pipeline × 3 provider adapters** | The only high-volume path; already heavily engineered |
| Local model inference | **4 classes** | OCR detection, OCR recognition, lettering erase, title embedding |
| Browser-direct local model | **1 path** | User's own loopback endpoint, deliberately never proxied |

The 14 server-side call sites are, by feature:

| # | Feature | Call shape | What it already guards |
| --- | --- | --- | --- |
| 1 | Plugin generation | **Loop**, ≤5 rounds, one tool, streamed | Structural code check, formatter pass, live progress |
| 2–3 | Login-mechanism analysis | **Loop**, ≤6 rounds, one tool; plus one JSON-reformat fallback round | Time bound, reformat-on-unparsable |
| 4 | Trial-run failure attribution | Single call | Degrade to "not relevant" |
| 5 | Volume-split suggestion | Single call | **Deterministic full-coverage validator**, but failure is terminal |
| 6 | Subfolder→tankoubon naming | Single call, batched | Silent fallback to folder names |
| 7–9 | Artist/cosplayer tag backfill | Three single calls (two paths) | Never overwrite, pre-write re-check, name cleanup |
| 10–11 | Tankoubon / chapter rename | Two single calls | None — model output passed through |
| 12 | Recommendation rerank | Single call over ~100 candidates | **Id allowlist, shortfall fill, fallback to embedding order** |
| 13 | Natural-language→subscription condition | Single call | **Constrained dialect + deterministic parser + field/operator allowlist** |
| 14 | Provider price-table extraction | Single call, truncated input | Singleflight, day cache, stale-value fallback |

### What the gap actually is

Measured against the agent-harness construct list, four constructs genuinely apply and are missing,
one applies partially, and six are structurally inapplicable:

**Missing and applicable:** the tool gating pipeline (pre-execute policy + deny-when-undecidable
approval), the tool-result pruning/spill step, the "model-visible is recorded" session invariant,
and the provider-neutral model-call seam (request retry, failure normalization, token metering).

**Partially present:** bounded repair. One call site has a hand-written JSON reformat round, one has
a deterministic validator that never feeds back, and the wizard's real verify→repair loop lives in
the browser, not the server.

**Structurally inapplicable** (recorded so this is not re-litigated): durable goals/plans/todos,
subagent delegation and workflow fan-out, conversation compaction, reusable instruction sets and
external hooks, and per-agent tool scoping. None of LANrurugi's AI features is an agent pursuing an
open-ended objective across turns — the only loop-shaped feature is a single bounded generation
task driven by a human in a wizard UI. Applying those constructs here would be over-engineering,
not hardening.

### Why now

The plugin wizard's page-fetch tool is the only place where the model chooses a network destination
*and* the server attaches real user credentials to it, and it does so with no destination
constraint. Page content fetched that way is fed back into the model's context. That is a
prompt-injection-to-credential-exfiltration path, and it sits on the wrong side of this project's
own secrets-and-network-trust-boundary principle. It is also the single largest concentration of
missing harness scaffolding in the codebase, so fixing it correctly fixes the pattern, not just the
instance.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - A credential is only ever sent to the site it belongs to (Priority: P1)

An administrator types their credentials for a site into the plugin wizard. During generation, the
model asks the server to fetch pages — sometimes several, sometimes on hosts the administrator never
mentioned. Whatever the model asks for, and whatever a fetched page tries to talk the model into
asking for next, the administrator's credential must never leave the server addressed to a site the
credential was not entered for.

**Why this priority**: this is the only security defect in the inventory. Every other story here is
quality and cost; this one is disclosure of a user secret to a third party, and it is reachable
through ordinary use of a shipped feature. It is also the only story whose failure mode is silent —
nothing in the UI would ever show that it happened.

**Independent Test**: with credentials entered for site A, drive generation against a scenario whose
model requests a fetch of site B carrying a credential placeholder. Delivered value: the fetch is
refused locally, the refusal is legible to the user, and no request reaches site B. Fully testable
without touching any other story.

**Acceptance Scenarios**:

1. **Given** credentials entered for site A, **When** the model requests a fetch of site A with a
   credential placeholder in a header, **Then** the fetch proceeds with the real value substituted,
   exactly as today.
2. **Given** credentials entered for site A, **When** the model requests a fetch of site B with a
   credential placeholder in a header, **Then** the fetch is refused before any network traffic, and
   the model receives a refusal it can reason about rather than a silent empty result.
3. **Given** credentials entered for site A, **When** a fetch of site A redirects to site B, **Then**
   the redirect is not followed with the credential attached.
4. **Given** no credentials have been entered at all, **When** the model composes a header containing
   a credential placeholder, **Then** the placeholder is not silently emptied into a request; the
   outcome is the same legible refusal as scenario 2.
5. **Given** the model requests a destination on the server's own host or a link-local/private
   address, **When** the request is evaluated, **Then** it is refused without being attempted.

---

### User Story 2 - No AI feature can hang or fail ambiguously (Priority: P2)

An administrator clicks an AI action. Whether the provider is slow, the connection stalls after the
request is accepted, or the provider answers with a transient error, the action reaches a definite
outcome — success, or a specific, explainable failure — within a documented bound. It never spins
indefinitely, and it never reports a generic failure where the real cause is knowable.

**Why this priority**: today only the two wizard actions have a bound at all; the other twelve call
sites can wait forever on a stalled connection. A hung action is indistinguishable from a slow one,
which makes the whole AI surface feel unreliable and leaves the administrator unable to tell whether
the work was billed.

**Independent Test**: force the provider to stall after accepting a request, and to return a
transient error, and confirm every AI action reaches the documented bound with a cause-specific
message. Delivered value: no AI action can hang, and the reason is always one of a small, listed set.

**Acceptance Scenarios**:

1. **Given** any AI action, **When** the provider accepts the request and then sends nothing,
   **Then** the action terminates with a timeout outcome by the documented bound, and no further
   requests are issued for that action.
2. **Given** any AI action, **When** the provider answers with a transient failure, **Then** the
   action retries once within its own bound and, if the retry also fails, reports that specific
   failure class.
3. **Given** any AI action, **When** the provider answers with an authentication failure, an
   exhausted-balance failure, or a rate-limit failure, **Then** each is reported as its own distinct,
   actionable class — never folded into a generic failure — and an authentication or balance failure
   is not retried.
4. **Given** a long-running multi-round action, **When** an individual round is retried,
   **Then** the action's overall bound still applies and is not silently extended.

---

### User Story 3 - The administrator can see what the AI features consumed (Priority: P3)

An administrator wants to know what each AI feature has cost them, in requests and tokens, and which
one is slow. They can answer that from the product surface, per feature, over a recent window,
without reading server logs or reconstructing it from provider invoices.

**Why this priority**: fourteen call sites currently record nothing, while one feature (translation)
records everything. That asymmetry means the answer to "what is my AI spend" is structurally
unavailable for everything except the one feature that already has a budget ledger.

**Independent Test**: exercise several AI features, then read the per-feature figures from the
product surface and confirm they match the requests actually issued. Delivered value: cost and
latency are attributable per feature.

**Acceptance Scenarios**:

1. **Given** any AI action completes, **When** the administrator inspects usage, **Then** that
   action's request count, token count (where the provider reports it), and elapsed time are
   attributed to the feature that issued it.
2. **Given** a request whose provider reports no token usage, **When** the administrator inspects
   usage, **Then** the request is counted but the token figure is reported as unavailable rather
   than as zero.
3. **Given** a window in which a feature issued no requests, **When** the administrator inspects
   usage, **Then** that feature reports "no data" rather than a misleading zero-cost line.
4. **Given** the server restarts, **When** the administrator inspects usage over a window that spans
   the restart, **Then** the figures for the whole window remain available.

---

### User Story 4 - An oversized page cannot swallow the generation's budget (Priority: P4)

During plugin generation the model may fetch a large, markup-heavy page. The administrator expects
the wizard to keep working and to keep costing roughly what an ordinary generation costs — not to
fail, stall, or spend its entire allowance on one page that was 95% navigation chrome.

**Why this priority**: tool results currently enter the model's context unbounded, and a multi-round
generation can accumulate several of them. This is the one remaining harness context-management
construct that applies to this codebase, and it directly determines whether a wizard generation
completes within a usable cost and time.

**Independent Test**: run generation against a scenario whose pages are an order of magnitude larger
than a typical page, and confirm the generation completes with per-round context bounded. Delivered
value: large pages degrade in fidelity, not in completion.

**Acceptance Scenarios**:

1. **Given** a fetched page far larger than the context budget, **When** the tool result is assembled,
   **Then** what enters the model's context is bounded, and the model is told the content was
   truncated and how much was withheld.
2. **Given** the model needs the withheld remainder, **When** it requests it, **Then** it can obtain a
   bounded further portion without re-fetching the whole page.
3. **Given** several large pages in one generation, **When** the rounds accumulate, **Then** the
   total context stays under its budget and the generation still terminates with either code or a
   specific failure.
4. **Given** a page within the budget, **When** it is fetched, **Then** its content is delivered
   unchanged from today.

---

### User Story 5 - A model output that provably fails is repaired once, not looped on (Priority: P5)

When a model produces output that a deterministic check proves wrong — a split suggestion that
leaves files unassigned, a rename list that does not match the members it was given — the
administrator expects one bounded, informed retry that tells the model exactly what was wrong,
rather than an immediate dead end or an open-ended self-correction loop.

**Why this priority**: the codebase already has the validators but not the loop (one validator never
feeds back, one rename path has no validator at all), and already has the loop but in the wrong tier
(the wizard's repair cycle is driven by the browser). Closing this is what turns "check" into
"correct".

**Independent Test**: feed a scenario whose first output fails a deterministic check and whose second
succeeds, and confirm exactly one repair attempt carrying the specific defect, with both attempts
recorded. Delivered value: provable failures become recoverable without manual retries.

**Acceptance Scenarios**:

1. **Given** output that fails a deterministic check, **When** the repair attempt is issued, **Then**
   the specific defect (not a restatement of the task) is included in what the model sees.
2. **Given** a repair attempt also fails, **When** the action terminates, **Then** it reports failure
   after exactly one attempt, never a second or further attempt.
3. **Given** output that passes its deterministic check, **When** it would still be semantically
   wrong, **Then** no repair attempt is issued — the bound is the deterministic check's verdict, not
   a judgment about quality.
4. **Given** a wizard generation whose trial run fails, **When** the failure is attributable, **Then**
   the attempt and its attribution are recorded together, whether the retry was requested by the user
   or offered by the system.

---

### User Story 6 - A generation can be reconstructed after the fact (Priority: P6)

An administrator reports "the wizard wrote a plugin that does not work". A maintainer can recover
exactly what the model was shown at each round — the prompts, every tool call, and every tool result
it acted on — without asking the administrator to reproduce it live.

**Why this priority**: lowest of the six, because it changes no user-visible behavior, but it is the
construct that makes every other story debuggable. Today a generation's model-visible inputs exist
only in a single-use in-memory slot and are destroyed on first read.

**Independent Test**: run a generation, restart the server, and recover the full sequence of
model-visible inputs for that generation. Delivered value: an unreproducible report becomes a
reviewable record.

**Acceptance Scenarios**:

1. **Given** a completed generation, **When** the server has restarted since, **Then** the sequence
   of model-visible inputs for that generation is still recoverable.
2. **Given** a failed attempt within a generation, **When** the record is inspected, **Then** the
   failure is preserved as a distinct attempt without appearing as something the model was told.
3. **Given** a credential value was substituted into a request header, **When** the record is
   inspected, **Then** the secret is not present in it in cleartext.
4. **Given** records accumulate over months of use, **When** they age past their retention window,
   **Then** they are discarded without affecting any other feature's data.

---

### Edge Cases

- A credential is entered for a site, and the model requests a fetch of a **subdomain** or a
  **different port** on that site. The boundary must be stated as a definite rule, not left to
  judgment.
- A fetch of an allowed host redirects to a second allowed host, then to a disallowed one. Every hop
  is evaluated, not just the first.
- Credentials exist for two sites in the same session and the model composes one request that
  references placeholders belonging to both.
- The provider reports a rate limit together with a retry-after hint that exceeds the action's own
  bound.
- A retry happens after a partial stream has already been delivered to the user; what the user saw
  must not contradict what is recorded.
- Token usage is reported on some rounds of a multi-round action and not others.
- A tool result is within the per-page budget but the accumulated rounds are not.
- A deterministic validator rejects an output for a reason the model cannot act on (for example, the
  assignment it must produce may be genuinely impossible for the given input).
- The same generation is resumed after a restart that happened between rounds.

## Requirements *(mandatory)*

### Functional Requirements

**Outbound request confinement (US1)**

- **FR-001**: The system MUST evaluate every model-requested fetch against an allow rule before any
  network traffic is issued, and MUST deny by default when the rule cannot be evaluated.
- **FR-002**: A request MUST NOT carry a substituted credential value unless its destination is the
  site the credential was entered for.
- **FR-003**: The allow rule MUST be re-evaluated on every redirect hop, not only on the initial
  destination.
- **FR-004**: The system MUST refuse destinations resolving to the server's own host, loopback,
  link-local, or private address space.
- **FR-005**: A denied request MUST produce an outcome the model can read and reason about; it MUST
  NOT appear to the model as an empty or unsuccessful-but-valid response.
- **FR-006**: A request whose header references a credential that does not exist MUST be denied
  rather than sent with the placeholder emptied or passed through literally.

**Bounded, unambiguous failure (US2)**

- **FR-007**: Every model call MUST carry a bound that applies to the whole action, including any
  retries, and MUST NOT depend on the transport's own default behavior.
- **FR-008**: Transient failures MUST be retried at most once per call, and the action's overall
  bound MUST still hold across the retry.
- **FR-009**: Authentication failures, exhausted-balance failures, and rate-limit failures MUST each
  be reported as distinct classes; authentication and balance failures MUST NOT be retried.
- **FR-010**: Every model call MUST report its outcome through one consistent failure vocabulary, so
  no call site has to interpret provider-specific detail to decide whether to retry, degrade, or
  abort.
- **FR-011**: An action that exhausts its bound MUST report a timeout outcome distinguishable from a
  provider error and from a validator rejection.

**Usage attribution (US3)**

- **FR-012**: The system MUST attribute every model call to the feature that issued it.
- **FR-013**: The system MUST record, per attributed call, request count, elapsed time, and token
  usage when the provider reports it, and MUST distinguish "not reported" from zero.
- **FR-014**: Attributed usage MUST remain queryable across a server restart over the retention
  window.
- **FR-015**: The administrator MUST be able to read attributed usage per feature from the product
  surface over at least a daily and a weekly window.

**Bounded tool results (US4)**

- **FR-016**: A tool result entering model context MUST be bounded, and when bounded the model MUST
  be told that content was withheld and how much.
- **FR-017**: The model MUST be able to obtain a further bounded portion of withheld content without
  re-fetching the source.
- **FR-018**: The accumulated context of a multi-round action MUST stay under its budget; when the
  budget is reached the action MUST terminate with a specific outcome rather than silently
  proceeding.
- **FR-019**: A tool result under the budget MUST be delivered unchanged.

**Bounded repair (US5)**

- **FR-020**: Where a deterministic validator exists, a failure MUST trigger at most one repair
  attempt that carries the specific defect into the model's next input.
- **FR-021**: No path MAY retry a deterministic-validation failure more than once, regardless of the
  action's remaining time or rounds.
- **FR-022**: A wizard trial-run failure that is attributable MUST be recordable together with the
  attempt it belongs to, independent of whether the repair was requested by the user or offered by
  the system.
- **FR-023**: A model output that no deterministic validator covers MUST NOT acquire a repair loop
  merely for being judged poor; absence of a validator is absence of a repair path.

**Reconstructibility (US6)**

- **FR-024**: For every action that issued at least one model call, the system MUST persist the
  sequence of model-visible inputs — prompts, tool calls, tool results — such that it can be
  recovered after a restart.
- **FR-025**: A failed or retried attempt MUST be recorded as a distinct attempt and MUST NOT be
  represented as content the model was shown.
- **FR-026**: Persisted records MUST NOT contain cleartext credential values.
- **FR-027**: Persisted records MUST expire on a retention window and their expiry MUST NOT affect
  any other feature's data or any user-visible behavior.

### Key Entities

- **Model Call**: one request to a model provider, attributed to the feature that issued it, with an
  outcome class, elapsed time, and optional token figures.
- **Attributed Usage**: the per-feature aggregation of Model Calls over a window, distinguishing
  reported from unreported token figures.
- **Tool Request**: a model-initiated request for the system to act (currently: fetch a page),
  carrying a destination and optional headers, evaluated against the allow rule before execution.
- **Tool Result**: the bounded outcome of a Tool Request, with an explicit truncation disclosure when
  content was withheld.
- **Attempt**: one model-visible round within an action, including rounds that failed or were
  retried, recorded as distinct from content the model was shown.
- **Generation Record**: the ordered set of Attempts for one generation action, retention-bounded,
  secret-free.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: In a scripted suite covering every credentialed outbound path, 100% of requests whose
  destination differs from the credential's own site are refused before any traffic leaves the
  server, and 0% of same-site requests change behavior.
- **SC-002**: 0 AI actions can exceed their documented bound under a forced-stall scenario, across
  all 9 AI features (today: 2 of 9 are bounded).
- **SC-003**: 100% of provider failure outcomes map to one of the listed classes; a scripted suite
  asserts that no failure path produces a generic message where a class is knowable.
- **SC-004**: An administrator can answer "requests, tokens and elapsed time for feature X over the
  last 7 days" from the product surface alone, in under 30 seconds, for 100% of the 9 features
  (today: 1 of 9).
- **SC-005**: For a generation whose pages are an order of magnitude larger than typical, per-round
  model-visible context stays within budget and the generation completes; measured request size does
  not grow proportionally to page size.
- **SC-006**: 100% of deterministic-validator failures produce exactly one repair attempt in a
  scripted suite; 0 repair loops exceed one attempt.
- **SC-007**: For 100% of generations in a scripted suite, the full sequence of model-visible inputs
  is recoverable after a restart, and a scan of the recovered records finds no cleartext credential
  value.
- **SC-008**: No shipped AI feature's ordinary-path behavior, output, or cost changes except where a
  story above explicitly requires it — a regression suite over the 14 call sites passes unchanged
  for the success paths.

## Assumptions

- **The harness definition used here is the agent-harness one**: the non-model scaffolding that makes
  a model-driven process bounded, observable, steerable and reproducible. Constructs are adopted only
  where one of LANrurugi's AI features actually has the shape that construct serves. "Makes AI more
  robust" alone is not sufficient grounds, and general service engineering (log verbosity, config
  tidiness, prompt versioning, background-job conversion) is tracked separately rather than folded
  in here.
- **The construct list is taken from the project's own recorded architecture reference**, the
  locally cloned agent harness already cited by the OCR/translation spec's research notes.
- **Six constructs are out of scope because no feature has their shape**: durable goals/plans/todos,
  subagent delegation, workflow fan-out, conversation compaction, reusable instruction sets and
  external hooks, and per-agent tool scoping. Deferring them is a design decision recorded here, not
  an oversight; re-opening it requires a feature that is actually long-running and open-ended.
- **The allow rule for outbound fetches is expressed in terms the wizard already has**: the site the
  credentials were entered for. No new user-facing configuration is introduced; if a rule cannot be
  derived from what the user already supplied, the request is denied.
- **Retention for generation records is a bounded window chosen at implementation time**, defaulting
  to a few weeks — long enough to answer a real bug report, short enough not to become an audit log.
- **Translation is not re-engineered by this feature.** It already implements the bounded-usage,
  pruning, and failure-taxonomy constructs it needs; only the shared call seam and usage attribution
  may touch it, and only additively.
- **Single-process, single-tenant deployment continues**: no per-user attribution, no multi-tenant
  quotas, no cross-user isolation work is implied by "usage attribution".
- **Local model inference and the browser-direct path are out of scope**: they already have per-call
  bounds, error classification, and lifecycle management.
