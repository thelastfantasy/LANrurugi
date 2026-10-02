# Contract: Discovery Capability

The one genuinely new extension surface this feature introduces. Extends the existing extension
protocol; transport, sandboxing, and permission model are unchanged.

## Why this exists

Existing entry points act on one already-known item: metadata enrichment takes an archive, download
takes a URL. Neither can answer *"what is new on this site that matches these criteria?"* — which is
the entire premise of a subscription. This capability answers exactly that question and nothing more.

## Shape

An extension MAY export a discovery function. Absence is normal and means the source cannot back a
subscription (FR-002b) — it is not an error.

**Input** — what to look for:

- The subscription's criteria: creator and/or tags.
- Optionally, a user-supplied listing URL (a feed or an index page).

Exactly one of the two drives a given call: an extension that knows how to search its own site is
given criteria; one pointed at a listing is given the URL. Both paths return the same thing, which is
why they share one capability rather than being two.

**Output** — what was found:

- A list of candidates, each carrying at minimum its source URL, plus whatever the listing cheaply
  revealed (title, posted time, rating, tags) so the orchestrator can apply filters without fetching
  every candidate individually.
- An explicit indication of whether the result is **complete** or **degraded**.

## Rules

1. **Report, do not decide.** The capability returns what the site says exists. It does not consult
   the library, does not deduplicate against prior checks, and does not decide what is worth
   downloading. The orchestrator owns all of that — one tested implementation shared by every source,
   rather than each extension reinventing it at varying quality. This mirrors the split already used
   for revision history.

2. **Source URLs must be normalised** the same way the rest of the system normalises them, so the
   orchestrator's duplicate check is a plain comparison. An un-normalised URL silently fails to match
   an identical work already held, with nothing reporting the mismatch.

3. **Degraded results must be declared, not inferred.** When a listing was fetched without the
   expected signed-in state, the result MUST say so. This is the single most important rule here: on
   real sources a signed-out search *succeeds* and simply returns less. Silence would be
   indistinguishable from "nothing new", and the orchestrator would mark unseen works as handled
   forever.

4. **Sign-in is declared, not embedded.** The extension states which login it depends on; the host
   supplies the signed-in state per call. Extensions do not hold credentials.

5. **Interval guidance is declared alongside.** The extension states a suggested check interval and a
   minimum one. The minimum is a floor the orchestrator enforces (FR-004b) — it encodes what the site
   tolerates, which the user has no way to know.

6. **Failure is reported, never thrown away.** An unreachable source, a changed page shape, or a
   rejected sign-in must come back as a stated reason. "Found nothing" and "could not look" are
   different answers and must remain distinguishable.

## Authoring assistant

The assisted authoring flow MUST be able to generate this capability (FR-027), and its list of
generatable capabilities MUST come from one definition rather than several hardcoded copies (FR-028).
A generated capability MUST be verifiable before saving (FR-029) — discovering it is broken at the
first scheduled check, hours later and unattended, is the failure mode to avoid.
