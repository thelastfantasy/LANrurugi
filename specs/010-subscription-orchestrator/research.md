# Phase 0 Research: Subscription Orchestrator

No `NEEDS CLARIFICATION` markers entered this phase — nine were resolved during
`/speckit-clarify` and are recorded in the spec's Clarifications section. This document captures the
decisions behind the design, including several where checking the codebase overturned the obvious
assumption.

## 1. Where candidate discovery lives

**Decision**: A new optional capability on the existing extension runtime. Given a subscription's
criteria it returns candidate works; the orchestrator does the rest.

**Rationale**: No current entry point can answer "what is new on this site" — `execMetadata` enriches
one known archive and `execDownload` fetches one known URL. Something must be built. Putting it in
the extension keeps site knowledge where site knowledge already lives, and means adding a source
later needs no orchestrator change. This is the same split `version_history` used successfully in
issue #107: the extension reports what the site says, the host decides what it means.

**Alternatives considered**:
- *Host builds search URLs per site.* Rejected: every new source would require host code, inverting
  the project's established division of labour.
- *User supplies a URL for every subscription.* Rejected as the only mechanism — it pushes work onto
  the user for sources an extension could handle unaided. Kept as one of two accepted inputs
  (FR-002c), because it covers sites no extension targets.

## 2. Parsing listings — no new interface needed

**Decision**: Reuse the parser the extension runtime already has.

**Rationale**: The dispatcher already embeds an HTML/XML parser with a CSS-subset selector API and an
XML mode for case-sensitive documents. RSS and Atom are XML. An extension can therefore read a feed
or a listing page with what it already has.

**Why this is worth recording**: A dedicated "parser plugin interface" was proposed and investigated.
Checking the runtime first showed the capability already existed, which turned a proposed new
interface into a confirmation that the existing one suffices. The real gap was *discovery*, not
parsing — a distinction easy to miss when both are described as "reading a page".

## 3. Sign-in: reuse, and treat its absence as inconclusive

**Decision**: Discovery uses the existing login mechanism, with credentials staying per extension
namespace. A check lacking the expected signed-in state is recorded as inconclusive, not as a
completed check.

**Rationale**: Extensions already declare which login they depend on and the host injects the
resulting state into each call — discovery rides that path unchanged, so a source the user can
already download from is one they can already subscribe to.

The second half matters more than it first appears. On real sources, searching signed-out returns a
*different, smaller* result set rather than an error. A check run without sign-in would therefore not
fail; it would succeed against a smaller world. Were its results treated as authoritative, works it
never saw would be marked seen-and-handled and never reconsidered — a silent, permanent omission that
no error message would reveal. Hence FR-002g/h.

**Alternatives considered**:
- *Per-subscription credentials.* Rejected: multiple subscriptions on one source already share an
  identity, and introducing per-subscription credentials would add a second secret-handling surface
  (constitution Principle V) to support "several accounts on one site", which this feature does not
  need.

## 3a. Bot challenges are a hard failure, not a degraded check

**Decision**: A discovery extension must detect an interposed bot challenge and report it as an
**error**, never as `degraded` and never as a successful empty listing.

**Rationale**: Verified live against e-hentai.org (2026-10-04): the site answers from behind
Cloudflare (`server: cloudflare`, `cf-ray` present). A challenge can arrive two ways, and only one of
them is safe by default:

- **HTTP 403/503** — caught by the existing `!response.ok` check, reported as an error, cycle recorded
  as `Failed`, nothing marked seen. Safe.
- **HTTP 200 carrying a challenge page** — the request "succeeds", the gallery-link pattern matches
  nothing, and a sign-in check looking only for the *site's own* login prompt sees nothing wrong. The
  cycle would be recorded as `Completed` with zero candidates, and **every work it never saw would be
  marked seen and permanently hidden**. This is the exact failure §3 guards against, reached by a
  route §3 did not cover: not signed out, but never served the listing at all.

Reported as an error rather than `degraded` because `degraded` means "a real answer to a smaller
question" — a challenge page is not a smaller answer, it is no answer. Treating it as degraded would
still surface its zero candidates as findings.

**Alternatives considered**:
- *Solving the challenge.* Rejected outright: out of scope, and an explicit non-goal.
- *Inferring it from an empty result.* Rejected — indistinguishable from a genuinely empty search,
  which is a legitimate outcome that must stay distinguishable (FR-002f).

## 3b. Listings must supply the fields the host filters on

**Decision**: A discovery extension returns `title`, `rating`, and namespaced `tags` whenever its
listing carries them — not just `source`.

**Rationale**: The host's tag rules are evaluated against `candidate.tags`. An extension returning
only `source` therefore makes **every required-tag rule reject everything**, and the rejection is
recorded as "missing required tag X" — indistinguishable, to the user, from a rule that is merely too
strict. Verified live: e-hentai's listing carries all three per gallery row (25 rows, 25 titles, 25
ratings, 7–12 tags each), so the data was available and simply unread.

Fields are read **per row** (the `gl2c` container), not by scanning the page once per field: the
latter pairs the Nth title with the Mth rating as soon as any row omits one, and the host would then
filter one work on another work's tags — a silent mis-filter with no failing test.

## 4. Scheduling: interval ownership and catch-up

**Decision**: The extension declares a suggested interval and a hard minimum; the subscription picks
anything at or above that minimum. Missed checks collapse into exactly one catch-up run.

**Rationale**: The two layers hold different knowledge and neither alone suffices — the extension
knows what its site tolerates, the user knows how closely they want to follow something. This mirrors
the existing "extension declares a default, user may override" settings shape, with the addition of a
floor: being rate-limited or banned is not a consequence the user should discover by trial.

Catch-up is capped at one run because checks are idempotent — replaying each missed interval would
see the same candidates while multiplying load and credit use at the exact moment a server comes
back up.

**Alternatives considered**:
- *Silently clamping an too-short interval.* Rejected: a user who believes they set five minutes but
  is actually getting sixty will misread every subsequent result. Refuse and explain instead.
- *Replaying every missed interval.* Rejected per the idempotence argument above.

## 5. Duplicate detection

**Decision**: Before downloading, compare normalised source identity; after downloading, compare
content. Works that are newer revisions of held items go through the existing revision comparison
rather than being skipped as duplicates.

**Rationale**: Source identity alone is cheap and available before spending anything, but it cannot
see that two URLs are the same bytes. Content alone is authoritative but only after the download —
too late to avoid the cost. Using each where it is strong gives a cheap pre-filter and a correct
backstop.

The revision clause exists because of issue #107: the same work's revisions have *different* source
URLs. Treating source identity as the only signal would make every new revision look like an
unrelated new work, while treating any related work as a duplicate would make subscriptions blind to
updates. The existing revision logic already distinguishes these cases, so this feature defers to it
rather than inventing a third answer.

## 6. Unattended downloading

**Decision**: Per-subscription choice, defaulting to *wait for confirmation*.

**Rationale**: Downloading consumes finite, real resources at the source. A newly created
subscription's rules are usually still being tuned — precisely when an over-broad rule would do the
most damage unattended. Defaulting to confirmation makes the dangerous mode opt-in, while leaving
full automation available once the user trusts their rules. This matches the posture already taken
for older-revision downloads in #107, which default to asking first.

## 7. Credit/budget handling is reactive, not predictive

**Decision**: React to a failure that indicates insufficient credit; do not attempt to predict or
display a balance.

**Rationale**: Verified against the codebase — there is no way to query a balance in advance; the
only available signal is the failure text a download returns. A requirement to check budget
beforehand would be untestable because the information does not exist. Specifying what the system can
actually observe keeps the requirement verifiable.

## 8. Authoring assistant must cover discovery

**Decision**: Extend the assisted authoring flow to generate the discovery capability, and consolidate
its capability list into one definition.

**Rationale**: Discovery is exactly the capability a new subscription source needs most. If the
assistant cannot produce it, every new source still has to be hand-written — defeating the point of
having an assistant. Verified during clarification that the flow enumerates its capabilities as
hardcoded literals in more than one place, so this is real work rather than something that follows
automatically.

## 9. Scheduling mechanism

**Decision**: Reuse the periodic-task pattern already used elsewhere in the server process; no new
dependency, no cron library, no separate process.

**Rationale**: The workspace already has the timer and time-handling libraries this needs, and the
server already runs several periodic sweeps on the same pattern. Per-subscription intervals differ
from those fixed sweeps, but that is a matter of computing the next due time rather than of needing
different machinery. Constitution Principle III rules out a separate process regardless.

**Open design detail for Phase 1**: whether to drive all subscriptions from one timer that wakes to
find what is due, or one task per subscription. Resolved in `data-model.md` in favour of a single
timer — per-subscription tasks would multiply idle tasks for no benefit at personal-library scale and
make non-overlap harder to reason about.
