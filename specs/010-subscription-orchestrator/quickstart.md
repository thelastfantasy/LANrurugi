# Quickstart: Validating the Subscription Orchestrator

How to prove this feature actually works. Each scenario maps to a user story and is independently
runnable — you can stop after any one of them and have validated something real.

## Prerequisites

- A running dev container (`mise run dev-up`); Rust changes need `mise run dev-rebuild` first,
  extension changes under `plugins/` take effect on the next call.
- At least one source extension offering the discovery capability.
- If that source needs sign-in, its login configured as usual — subscriptions use the same
  credentials as manual downloads, with nothing extra to set up.

## Scenario 1 — A subscription brings in new works (User Story 1)

1. Create a subscription in Settings → Subscriptions: pick the source, name a creator, choose a short
   interval, pick a target category.
2. Press **Check now** rather than waiting for the schedule.
3. Because confirmation-before-download is the default, matched works appear **awaiting approval**,
   not downloading.
4. Approve them. They enter the download queue and then the library, in the chosen category.

**Passes when**: works arrive with no URL ever typed by hand, and nothing downloaded before approval.

**Then verify the default is real** — before approving in a fresh run, confirm no download credit was
spent and no queue entry was created. This is the guard against an over-broad rule spending real
resources unattended (FR-007a).

## Scenario 2 — Automatic mode (User Story 1, opted in)

1. Turn on automatic download for that subscription.
2. **Check now** again, against a creator with works not yet held.
3. Matches go straight to the download queue without an approval step.

**Passes when**: the same subscription behaves differently purely because of that one setting, and
the queue entries are attributed to the subscription (FR-022).

## Scenario 3 — Nothing is downloaded twice (User Story 1)

1. Run **Check now** twice in a row on a subscription that already pulled works in.
2. Inspect the second cycle's result.

**Passes when**: the second cycle reports candidates as already-held or already-seen and queues
nothing. This is the behaviour that makes an unattended subscription safe to leave running.

**Also check the revision case** if the source has one: a work that is a *newer revision* of
something held must not be dismissed as a duplicate — it goes through the normal revision handling
instead. Skipping it would make subscriptions permanently blind to updates.

## Scenario 4 — Filters, and seeing why (User Story 2)

1. Add an exclude-tag rule matching something the creator actually publishes.
2. **Check now**.
3. Open the cycle's result.

**Passes when**: the excluded work is not queued **and** the result names the rule that rejected it.
A filter that silently drops things is indistinguishable from a broken subscription, which is why
FR-011 requires the reason.

## Scenario 5 — Failures surface instead of vanishing (User Story 3)

1. Make downloads fail — point a subscription at an unreachable source, or use a source where credit
   is exhausted.
2. **Check now**, then open the upload page.

**Passes when**: the reservation group shows the entry with its real reason, and the group was
collapsed (not absent) beforehand. Also confirm:
- Retrying after removing the obstacle completes the download.
- Discarding an entry means later cycles do not re-reserve the same work (FR-016).
- Each entry can reach the subscription that produced it (FR-026).

## Scenario 6 — A signed-out check is inconclusive, not empty (the subtle one)

Only applies to sources whose results differ by sign-in.

1. With a working subscription, remove or invalidate the source's sign-in.
2. **Check now**.

**Passes when**: the cycle reports **inconclusive** — not "0 new works". Then, critically:

3. Restore sign-in and **Check now** again.
4. Works that existed during the signed-out check are **still found**.

**Why this scenario matters more than it looks**: on real sources a signed-out search succeeds and
quietly returns less. Were the signed-out result treated as authoritative, every work it could not
see would be marked handled and never offered again — a permanent, silent omission that no error
message would reveal. Step 4 is the one that actually proves the guard works.

## Scenario 7 — Interval floor (FR-004b)

1. Try to set an interval shorter than the source's declared minimum.

**Passes when**: it is refused with an explanation. A silently raised interval would leave the user
misreading every later result, believing checks are more frequent than they are.

## Scenario 8 — Restart safety (FR-013)

1. With subscriptions configured and reservations present, restart the service.
2. Confirm subscriptions, their schedules, and reservation entries all survived.
3. Let a due subscription run.

**Passes when**: nothing is re-downloaded, nothing is lost, and a subscription whose schedule lapsed
during the downtime runs **once** on return — not once per missed interval (FR-008a).

## Scenario 9 — Budget behaviour (User Story 4)

1. Set a subscription to pause when credit runs out; trigger that failure.

**Passes when**: the subscription stops and is shown as paused *with the reason*, and resuming
restores normal scheduling. Then set it to continue instead and confirm failures are reserved while
checking carries on.

## What this does not cover

- Load behaviour at scale: this is a personal-library feature, and politeness to sources (the
  interval floor) matters more than throughput.
- Sources with no discovery capability: they are not selectable, which Scenario 1 already exercises
  by their absence.
