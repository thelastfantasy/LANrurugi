# Contract: Subscription & Reservation HTTP Surface

All paths are additive and sit outside the legacy path set, so no existing contract changes
(constitution Principle II). Shapes follow this project's existing conventions rather than
introducing new ones.

## Subscriptions

| Method & path | Purpose |
|---|---|
| `GET /subscriptions` | List all, with enough state to render the management section |
| `POST /subscriptions` | Create |
| `GET /subscriptions/{id}` | One subscription, including recent check history |
| `PUT /subscriptions/{id}` | Edit |
| `DELETE /subscriptions/{id}` | Delete |
| `POST /subscriptions/{id}/enable` · `/disable` | Turn checking on or off |
| `POST /subscriptions/{id}/resume` | Clear a system-applied pause (FR-020) |
| `POST /subscriptions/{id}/check` | Run now, without waiting for the schedule (FR-007) |

**Creation and edit validation**

- An interval below the source's declared minimum is **refused with its reason** (FR-004b), not
  silently raised. A user who thinks they set five minutes but is getting sixty will misread every
  later result.
- A source that offers no discovery capability is **not selectable**, and the response says why
  (FR-002b) rather than accepting the subscription and failing at the first check.

**Sources**

| Method & path | Purpose |
|---|---|
| `GET /subscriptions/sources` | Which sources can back a subscription, each with its suggested and minimum interval, and whether it needs sign-in |

This exists so the UI can present only viable sources and show interval bounds before the user
submits, rather than relying on a rejection to teach them the rules.

## Reservations

| Method & path | Purpose |
|---|---|
| `GET /reservations` | The list for the upload page (FR-017) |
| `POST /reservations/{id}/retry` | Retry one |
| `POST /reservations/retry_all` | Retry all waiting entries |
| `POST /reservations/{id}/discard` | Discard one — remembered, so later cycles do not re-reserve it (FR-016) |

Each entry carries the subscription that produced it, so the user can reach the rule responsible
(FR-026) instead of hunting for which subscription keeps generating failures.

## Preview and check-now

| Method & path | Purpose |
|---|---|
| `POST /subscriptions/{id}/preview` | What a check would do, without doing any of it |
| `POST /subscriptions/{id}/check` | Run a real check now instead of waiting for the schedule (FR-024) |

Preview takes an optional body carrying an unsaved subscription, so rules can be tried before being
committed — otherwise a rule would have to be saved to find out what it does. Only its `criteria` and
`filters` are honoured; the saved subscription's identity and source are kept, so a preview cannot be
used to point a subscription somewhere else.

It returns every candidate the source listed with each one's verdict, not only the matches. A rule
that is too strict looks exactly like a source with nothing new unless the works it turned away are
visible with their reasons.

Preview is explicitly allowed on a **disabled** subscription: tuning a rule is precisely when it is
switched off. It writes nothing — nothing queued, nothing marked seen, no snapshot, no cycle logged —
so it is safe to run repeatedly, costing one request to the source each time.

Check-now is accepted and runs in the background: a check contacts the source and may page through it,
which is longer than a request should hold open. Its result appears in the subscription's own history.

## Pending approvals

Needed because confirmation-before-download is the default (FR-007a/b).

| Method & path | Purpose |
|---|---|
| `GET /subscriptions/pending` | Matched works awaiting approval |
| `POST /subscriptions/pending/approve` | Approve some or all — only then is download credit spent |
| `POST /subscriptions/pending/dismiss` | Dismiss some or all |

Approval is the point where unattended spending becomes attended spending. Nothing before it may
consume credit.

Both actions take `{ "ids": [...] }` rather than an id in the path, so that acting on one match and
acting on a batch are the same call (FR-007b). Approve reports `approved` and a per-id `failures`
list instead of rolling the batch back: with several items, one that fails to queue is no reason to
un-approve the rest, and the caller needs to know *which* ones did not make it. An approved work
enters the ordinary download queue, inheriting its existing start/stop/retry behaviour; a dismissed
one is marked seen, so the same question is not raised next cycle.

## Cross-cutting

- **Attribution**: downloads started by a subscription are recorded distinguishably from
  user-initiated ones (FR-022), so an unattended download's origin stays auditable.
- **Errors**: failures carry a structured, translatable reason in the project's existing style —
  never a bare English string. "Could not look" must stay distinguishable from "found nothing".
- **Authorisation**: these are administrative endpoints and follow the same policy as other
  configuration surfaces; they are not part of any guest-visible surface.
