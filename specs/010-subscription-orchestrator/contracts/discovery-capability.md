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

7. **Filterable fields are typed on the result interface, not declared separately.** Which fields a
   subscription may filter on follows from the SDK's own candidate type plus static analysis of what
   the extension writes — the same two signals `version_history` already uses. See the next section.

## Filterable candidate fields

Declared the same way `DownloadResult.version_history` is: as a **typed field on the SDK's own result
interface**, annotated by the extension, checked at compile time. Not as a separate runtime
declaration the extension also has to keep in step.

**Why this shape.** The host deserializes a candidate into a Rust struct that skips anything it does
not recognise, so an unannotated return value has nothing checking it against the real contract — a
misspelled `postedat` is silently dropped and the filter that depends on it silently matches nothing.
Annotating `discover` with the SDK's result type (`): Promise<DiscoveryResultShape>`) makes that a
compile error instead. This is the exact reasoning `DownloadResultShape`'s own doc comment gives, and
the same guarantee is what filter rules need: a field the user can filter on must be a field the
extension actually populates.

**How the host knows which filters to offer.** From the same two signals it already uses for
`version_history`:

1. **The type**, from the SDK interface. Each field's declared type fixes which operators apply, so a
   declaration cannot ask for an operator the host has no implementation for, and every operator has
   one tested implementation shared by all sources.
2. **Static analysis of the plugin's source**, the mechanism behind
   `PluginIntrospection::returns_version_history` — whether the extension's `discover` body ever
   writes a given key. A freshly-edited extension therefore takes effect on the next options request,
   with no check ever having run. The alternative, observing it at runtime, would mean the UI could
   not offer a filter until a check had already happened.

**The fields, and the operators their types imply:**

| Field | Type | Operators | Meaning the host attaches to it |
|---|---|---|---|
| `posted_at` | date string | `older_than`, `newer_than` (a duration before now) | Hold a work back until a chosen time after publication |
| `rating` | number (0–5) | `gte`, `lte` | A rating floor. **Absent means not yet rated**, not zero |
| `tags` | string list | `includes_all`, `includes_none` | Namespaced verbatim as the source writes them |
| `category` | string | `in`, `not_in` | The source's own classification |
| `uploader` | string | `equals`, `contains` | Also what a creator-scoped subscription matches on |
| `pages` | number | `gte`, `lte` | Length, as a proxy for download cost |

**Why `date` operators are durations rather than timestamps.** A subscription is a standing
instruction. "Newer than 2026-10-01" means something different every day it runs and eventually matches
everything; "published more than three hours ago" means the same thing on every check.

**Why `posted_at` is a string and not a `Date`.** It crosses a JSON boundary. The host parses the
shapes a listing actually uses (`"YYYY-MM-DD HH:MM"`, RFC 3339, a bare epoch) and **fails open** on
anything it cannot read — treating the work as old enough. Failing closed would let one unrecognised
date format hold an entire catalogue back with nothing in the UI explaining why.

**Rules**

1. A filter may only be offered for a field the extension actually writes. Offering one over a field
   never populated would present a rule that silently matches nothing.
2. **An absent optional field is unknown, not failed.** A rule over it neither passes nor rejects; the
   work is held for reconsideration on a later check. A rejection is permanent, and the value may
   simply not exist yet — which is exactly the rating case, where a work minutes old has no rating and
   judging it then decides the question at the moment the answer is least knowable.
3. A rule referencing a field the extension has **stopped** writing is reported as inapplicable,
   carrying that reason — never dropped (which silently widens the subscription) and never treated as
   satisfied (which silently narrows it). Both are changes a user notices only by what fails to arrive.
4. An unrecognised field is ignored rather than rejected, so an extension written against a newer SDK
   stays loadable on an older host.

## Authoring assistant

The assisted authoring flow MUST be able to generate this capability (FR-027), and its list of
generatable capabilities MUST come from one definition rather than several hardcoded copies (FR-028).
A generated capability MUST be verifiable before saving (FR-029) — discovering it is broken at the
first scheduled check, hours later and unattended, is the failure mode to avoid.
