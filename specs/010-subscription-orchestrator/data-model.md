# Phase 1 Data Model: Subscription Orchestrator

All storage is additive: new key namespaces only, no existing key shape touched (constitution
Principle I). Namespaces follow the project's `LANRURUGI_*` convention.

## Entities

### Subscription

A standing instruction. Long-lived, edited rarely, read on every scheduling decision.

| Field | Meaning | Notes |
|---|---|---|
| `id` | Stable identifier | Newtype-wrapped, per the project's ID-type rule |
| `name` | What the user calls it | Free text, shown in lists and in reservation entries |
| `source` | Which extension discovers for it | Must currently offer discovery (FR-002b) |
| `criteria` | What it tracks | Creator and/or tags, plus an optional user-supplied listing URL (FR-002c) |
| `filters` | What to keep or drop | Required tags, excluded tags, minimum rating, excluded categories |
| `interval` | How often to check | Must be ≥ the source's declared minimum (FR-004b) |
| `target_category` | Where results land | Optional |
| `enrich_metadata` | Run metadata enrichment on its downloads | Default on |
| `auto_download` | Download without asking | **Default off** (FR-007a) |
| `on_insufficient_credit` | Pause, or continue and reserve | FR-019 |
| `on_source_changed` | What to do when a cached work's title/tags change at the source | **Default `notify_only`** — see below |
| `on_source_removed` | What to do when a cached work disappears from the listing | **Default `mark_only`** — see below |
| `state` | Enabled / disabled / paused | Paused carries its reason |
| `last_checked_at` | When it last ran | Drives both scheduling and catch-up |

**`on_source_changed` / `on_source_removed`: the user's call, not a built-in default**

Both default to the least destructive option, and both are per subscription rather than global: a
subscription tracking one trusted creator and one scraping a broad tag search warrant different
answers, and no single default is right for both.

`on_source_changed` — a cached work's title or tags differ from what the listing showed last time:

| Option | Behaviour |
|---|---|
| `notify_only` (default) | Record the change in the cache and surface it. Nothing else happens. |
| `refetch_metadata` | Additionally queue the held archive for metadata re-enrichment, so the change propagates through the existing metadata path. |
| `ignore` | Update the cache silently; do not surface anything. For a noisy source whose tags churn. |

**The cached tags must never be written to a held archive directly.** Verified live against
e-hentai.org (2026-10-04): a listing row truncates its tag list — 19 of 25 rows carried exactly 12
tags, which no real set of works would. Writing those onto an archive would *delete* real tags.
Cached tags exist to answer "does this still match the subscription", nothing more; the archive's
metadata has its own authoritative source (the metadata capability's full record) and its own
merge-never-delete rule. `refetch_metadata` therefore re-runs *that* path rather than copying cache
values across.

Titles are likewise never synced automatically, under any of the three options: the local title may
have been edited by the user (`PUT /archives/{id}/metadata` permits exactly that), and silently
overwriting their edit is worse than leaving a stale title. A change is surfaced with both values and
an explicit action to adopt the new one.

`on_source_removed` — a cached work is absent from a listing that previously carried it:

| Option | Behaviour |
|---|---|
| `mark_only` (default) | Mark it in the cache; withdraw it if it was still awaiting approval. Held archives are untouched. |
| `notify` | As above, plus surface it as something needing attention. |
| `ignore` | Forget it silently. |

**No option deletes a held archive.** Removal has innocent causes — a tag edited off, a temporary
takedown, a throttled response returning a partial listing — and deletion is irreversible. Marking is
offered; deleting stays a manual decision the user makes with the mark in front of them.

Marking is also gated: it requires the cycle to be authoritative (reusing `CycleOutcome::
is_authoritative`, so a failed, inconclusive, or challenge-blocked check never marks anything) **and**
the work to be absent for several consecutive checks. One missing appearance is not evidence; a
single bad response would otherwise mark a whole page of works at once.

**State transitions**

```
disabled ──enable──▶ enabled ──user pauses / credit exhausted──▶ paused
   ▲                    │                                           │
   └────disable─────────┴──────────────resume──────────────────────-┘
```

`paused` is distinct from `disabled`: the system paused it and records why, versus the user turned it
off. Both stop checks; only `paused` carries a reason to show and a reason to offer "resume".

### Field Rule

One rule the user built over a candidate field. Stored in the subscription's `filters`.

| Field | Meaning |
|---|---|
| `field` | Which candidate field it applies to, by name |
| `operator` | One of the operators that field's type allows |
| `value` | The comparison value, shaped by the type |

The set of filterable fields is **not** stored here. It follows from the SDK's own candidate type plus
static analysis of which keys the extension writes — the same two signals
`PluginIntrospection::returns_version_history` already uses. Storing a copy per subscription would be a
second, separately-derived answer to the same question, and the two would eventually disagree.

**Unknown is not failed.** When a candidate lacks an optional field, a rule over it neither passes nor
rejects: the work is held for reconsideration rather than rejected, because a rejection is permanent
and the value may simply not exist yet. The rating case is the clearest instance — a work minutes old
usually carries no rating, so judging it then would decide the question at the moment the answer is
least knowable. This is why a rating floor alone is not enough to express "download only what scored
well": it has to be paired with a waiting period, after which an absent rating becomes informative
rather than merely premature.

**A rule over a field the extension stopped writing** is reported as inapplicable, carrying that
reason. Dropping it would silently widen the subscription; treating it as satisfied would silently
narrow it. Both are changes the user notices only by what fails to arrive.

**Date operators are durations, not timestamps.** A subscription is a standing instruction: "newer than
2026-10-01" means something different every day it runs and eventually matches everything, while
"published more than three hours ago" means the same thing on every check.

### Check Cycle

One execution of one subscription. Append-only history; bounded by age or count so it cannot grow
without limit.

| Field | Meaning |
|---|---|
| `id`, `subscription_id`, `started_at`, `finished_at` | Identity and timing |
| `outcome` | Completed / failed / **inconclusive** |
| `candidates_seen` | How many the source offered |
| `per_candidate` | Each candidate's verdict (see below) |

**`inconclusive` is a first-class outcome, not an error.** It is what a check reports when it ran
without the expected signed-in state (FR-002h). An inconclusive cycle's candidate list is *not*
authoritative: it must not mark works as seen, because the source was answering a smaller question
than the one asked.

### Candidate Verdict

One work a cycle considered. Carried inside the cycle rather than stored separately — it has no life
of its own.

| Field | Meaning |
|---|---|
| `source_url` | Normalised, so the same work in different URL forms is one work |
| `verdict` | queued / reserved / rejected / already-held / already-seen |
| `reason` | For `rejected`, which rule rejected it (FR-011) |

### Reservation Entry

A matched work that could not be downloaded — the user's to-do list.

| Field | Meaning |
|---|---|
| `id`, `subscription_id`, `source_url`, `created_at` | Identity, provenance (FR-026), timing |
| `reason` | Why it could not download — unreachable, insufficient credit, failure |
| `status` | Waiting / discarded |

A discarded entry is **kept, not deleted**: later cycles consult it so the same work is not
re-reserved (FR-016). Deleting it would make the discard decision evaporate.

### Listing Snapshot Entry

One work as a listing last showed it. The cache that makes change detection possible at all, and the
list a user browses (issue #111).

| Field | Source | Meaning |
|---|---|---|
| `source` | listing | Primary key, already canonicalised |
| `title` | listing | What the listing calls it now |
| `posted_at` | listing | When the source says it was published |
| `tags` | listing | **Truncated by the listing — see below.** Used only to judge "does this still match" |
| `rating` | listing | Feeds the minimum-rating rule |
| `category` | listing | Feeds the excluded-category rule |
| `uploader` | listing | Who posted it; also what a creator-scoped subscription matches on |
| `pages` | listing | Length, for judging download cost before approving |
| `first_seen_at` | local | When *we* first saw it — distinct from `posted_at`, which is when the source published it |
| `last_seen_at` | local | Which check last showed it; the basis for the set difference |
| `disappeared_at` | local | Set only under `on_source_removed`'s gating rules |
| `missing_count` | local | Consecutive checks without it, so one bad response cannot mark a page of works |
| `no_longer_matching` | local | It is still listed, but its current tags no longer satisfy the subscription |
| `title_changed_from` | local | The previous title, kept so the change can be shown as old → new |

All eight listing fields were verified present and parseable on every row of a real e-hentai listing
(2026-10-04, 25/25 for each). They are read **per row**, from the row's own container, never by
scanning the page once per field — the latter mispairs the Nth title with the Mth rating as soon as
one row omits a field, and the subscription would then filter one work on another's tags.

`thumbnail_url` is deliberately **not** stored: thumbnail URLs expire, and rendering them as hotlinks
would put a request to the source on every browse, defeating one of the cache's purposes.

**Why `posted_at` is stored but is not how "new" is decided.** New-ness is decided by set membership
against `Seen Work`, never by `posted_at` being above a high-water mark. A tag-or-keyword search's
result set changes in three ways, and only the first is chronological:

1. a work is newly published — it appears at the top;
2. an **existing** work is edited to carry the tag — it appears *in the middle*, with a `posted_at`
   that may be years old;
3. a work loses the tag, or is deleted — it disappears.

A high-water mark handles (1) and systematically misses (2). `posted_at` is kept for ordering,
display, deciding how far back to page, and answering "why did a 2024 work only arrive today" — not
for deciding what to act on.

**Why this cannot reuse `Seen Work`.** `Seen Work` is written with `SADD`: an append-only accumulation
of everything ever seen. A snapshot of *what the last check showed* is a different thing, and the set
difference needs both — `previous − current` is what disappeared, `current − previous` is what is new.

### Pending Approval

A matched work waiting for the user's go-ahead, because its subscription has `auto_download` off
(the default, FR-007a).

| Field | Meaning |
|---|---|
| `id`, `subscription_id`, `source_url`, `created_at` | Identity, provenance, timing |
| `title` | What the user is deciding about, when the listing supplied one |

Its own record rather than a read back over cycle history: history is append-only and bounded by
age, so a pending item would silently expire out of it while still being, from the user's point of
view, an unanswered question. Distinct from `Reservation Entry` in *who is waiting on whom* — a
reservation is the system reporting it could not download something, a pending approval is the
system asking permission it has not yet been given.

Approving moves the work into the ordinary download queue and deletes the record, in that order: the
other order would lose the work entirely while the user believed they had approved it. Dismissing
writes `Seen Work`, so the same question is not asked again next cycle.

### Seen Work

The record that stops a work being proposed twice.

| Field | Meaning |
|---|---|
| `subscription_id` + `normalised_source` | Composite identity |
| `first_seen_at` | When |

Written only by **conclusive** cycles. This is the direct consequence of entity `Check Cycle`'s
`inconclusive` outcome: marking works seen from a signed-out check would permanently hide works that
check never had access to.

## Key Namespaces

| Purpose | Key shape |
|---|---|
| Subscription record | `LANRURUGI_SUBSCRIPTION_<id>` |
| All subscription ids | `LANRURUGI_SUBSCRIPTIONS` (set) |
| Cycle history | `LANRURUGI_SUBSCRIPTION_CYCLES_<subscription id>` (bounded list) |
| Seen works | `LANRURUGI_SUBSCRIPTION_SEEN_<subscription id>` (set) |
| Reservation entry | `LANRURUGI_RESERVATION_<id>` |
| All reservation ids | `LANRURUGI_RESERVATIONS` (set) |
| Pending approval | `LANRURUGI_SUBSCRIPTION_PENDING_<id>` |
| All pending-approval ids | `LANRURUGI_SUBSCRIPTION_PENDING` (set) |
| Listing snapshot | `LANRURUGI_SUBSCRIPTION_SNAPSHOT_<subscription id>` (hash, keyed by canonical source) |

Seen-works sets are per subscription, not global: two subscriptions may legitimately both want the
same work, and FR-010's "queue it once" is a per-cycle concern handled at queueing time, not by
making one subscription's history hide the work from another.

## Scheduling Model

**One timer, not one task per subscription.** The scheduler wakes periodically, asks which
subscriptions are due, and runs those. Per-subscription tasks would multiply idle tasks for no gain
at this scale and make FR-012's non-overlap guarantee harder to reason about — with a single
scheduler, "is this subscription already running?" is a local question.

**Due** means `last_checked_at + interval` has passed. A subscription whose due time passed while the
service was down is due exactly once on return, which gives FR-008a's single catch-up for free: the
rule is "due or not", never "how many times due".

**Non-overlap** (FR-012) comes from the scheduler never starting a subscription that is already
running. Since one scheduler owns all starts, no cross-task coordination is needed.

## Relationships

```
Subscription ──1:N──▶ Check Cycle ──1:N──▶ Candidate Verdict
     │
     ├──1:N──▶ Reservation Entry   (provenance: FR-026)
     ├──1:N──▶ Pending Approval    (cleared by approve → queue, or dismiss → Seen Work)
     ├──1:N──▶ Listing Snapshot    (what the last check showed; diffed to find new and removed)
     └──1:N──▶ Seen Work           (written only by conclusive cycles)
```

## Validation Rules

- `interval` ≥ the source's declared minimum; a shorter value is refused with its reason (FR-004b),
  never silently clamped.
- `source` must currently offer discovery; otherwise it is not selectable and the reason is shown
  (FR-002b).
- A subscription referencing a vanished category, extension, or source must surface that rather than
  failing silently at check time.
- Cycle history and reservation entries are both bounded (FR-018), and the user is told when entries
  were dropped for that reason rather than finding them quietly gone.
