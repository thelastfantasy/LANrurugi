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
| `state` | Enabled / disabled / paused | Paused carries its reason |
| `last_checked_at` | When it last ran | Drives both scheduling and catch-up |

**State transitions**

```
disabled ──enable──▶ enabled ──user pauses / credit exhausted──▶ paused
   ▲                    │                                           │
   └────disable─────────┴──────────────resume──────────────────────-┘
```

`paused` is distinct from `disabled`: the system paused it and records why, versus the user turned it
off. Both stop checks; only `paused` carries a reason to show and a reason to offer "resume".

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
