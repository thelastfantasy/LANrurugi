# Phase 0 Research: Agent-Harness Hardening

**Feature**: `014-ai-harness-hardening`

Every decision below is recorded as **Decision / Rationale / Alternatives considered**. The first
decision is the one the whole feature rests on; the rest are consequences of it.

---

## 1. Which harness constructs this codebase should adopt, and which it must not

**Decision**: Adopt five constructs — the tool gate, the model-call seam, usage measurement, tool-
result bounding, and bounded repair. Do not adopt six others: durable goals, plans and todos;
subagent delegation and workflow fan-out; conversation compaction; reusable instruction sets and
external hooks; per-agent tool scoping; and a generic agent-loop abstraction.

The test applied to each candidate was *shape*, not *benefit*: a construct is adopted only if at
least one feature in this codebase actually has the shape the construct exists to serve.

| Construct | Shape it serves | Does any LANrurugi feature have that shape? |
| --- | --- | --- |
| Tool gate | a model that chooses side-effecting actions | **Yes** — the plugin wizard's page fetch, and it is the only such place |
| Model-call seam | many call sites needing one consistent failure/retry/measurement contract | **Yes** — 14 call sites, 14 independently written error maps |
| Usage measurement | spend and latency that must be attributable | **Yes** — 9 features, 1 measured |
| Tool-result bounding | a loop that accumulates model-visible observations | **Yes** — one loop, with a fetcher that has no size limit |
| Bounded repair | deterministic validators that can inform a retry | **Yes** — two validators exist, one rename path has none |
| Goals / plans / todos | an objective pursued across turns, held by the system | **No** — the objective is held by the human in a wizard UI; the server is a callee |
| Subagent / workflow fan-out | parallel delegation of independent work | **No** — the only parallelizable AI work is tag backfill, which rate limits force to be sequential (already argued in-code) |
| Conversation compaction | history growing unbounded over a long session | **No** — each generation is one independent request; the translation pipeline has already recorded a reasoned rejection of append-only sessions |
| Skills / hooks | external instruction sources and external hook configs | **No** — single tenant, no external instruction provider, no hook config |
| Per-agent tool scoping | multiple agents with differing tool sets | **No** — one agent-equivalent, one tool |
| Generic agent loop | many loop-shaped call sites | **No** — two |

**Rationale**: the failure mode this guards against is not under-engineering, it is adopting a
framework for two users and then having to satisfy its invariants everywhere. Every construct above
carries a non-trivial invariant that would then become the codebase's invariant. Adopting one with no
shape to serve imports cost with no benefit, and — worse — makes the *actually needed* constructs
harder to reason about because they arrive wrapped in the same abstraction.

**Alternatives considered**:
- *Adopt everything portable, because it is all "best practice".* Rejected: three of the six
  rejected constructs (goals, subagents, workflows) require a session/agent lifecycle this product
  does not have. Building that lifecycle is a different project.
- *Adopt nothing, fix each defect in place.* Rejected for the model-call seam specifically: the
  fourteen error maps are demonstrably divergent (two different status-code ladders, one of which
  handles five classes and the other four), so fixing them in place leaves the next call site to
  invent a fifteenth.

---

## 2. One failure vocabulary, adopted without touching fourteen control flows

**Decision**: Introduce a single error type for model calls, with variants for "not configured",
"authentication", "exhausted balance", "rate limited (with an optional retry-after)", "transient
transport/server failure", "deadline exceeded", "malformed response", and "rejected by a
deterministic validator". Its human-readable rendering is byte-identical to today's messages, and it
converts into `String` so that a call site returning `Result<_, String>` keeps compiling unchanged.

**Rationale**: today the taxonomy is expressed only as formatted Chinese sentences built inside two
copies of a status-code ladder. Any call site that wants to branch on class must re-parse prose or
re-implement the ladder; none do, so all fourteen currently degrade identically regardless of cause.
A typed vocabulary with a lossless `Display` makes the class available for free and lets the retry
policy, the usage ledger and the HTTP error mapping each consume the same thing. Converting into
`String` is the whole adoption trick: no call site is forced to change, and each one can opt into
matching later. This is what makes a 14-site refactor land as a 14-line change.

**Alternatives considered**:
- *Full typed refactor of all fourteen signatures.* Rejected for this phase: it rewrites control flow
  at every call site and therefore every call site's tests, for no behavioral gain over the additive
  form. The additive form reaches the same end state incrementally.
- *Keep `String` and add a prefix convention.* Rejected: unchecked stringly-typed classes are exactly
  the current state, one layer down.

---

## 3. What "the credential's own site" means, precisely

**Decision**: The permitted destination set for a generation is computed once, from user input only:
the domain the user entered for this wizard session, plus the hosts of the test links and auxiliary
reference URLs the user supplied in the same request. Matching is by host, at DNS-label boundaries,
and also matches a subdomain of a permitted host (`img.example.com` is permitted when
`example.com` is). Scheme must be `http` or `https`. Port is not constrained. Nothing derived from
model output ever enters this set.

**Rationale**: three properties are load-bearing. (1) *Derived only from user input* — if the model
could widen its own permitted set, the gate would be theater. (2) *Label-boundary matching* — naive
suffix matching would accept `evil-example.com` for `example.com`, which is the single most likely
mistake in any hand-rolled host rule. (3) *Subdomains in* — real sites routinely redirect to and load
from subdomains, and a rule that fails them produces a gate administrators turn off.

Port is deliberately unconstrained: leaking a credential requires the attacker to control a port on
the user's own host, which is not this feature's threat model, and constraining it would break sites
served from non-default ports for no security gain.

**Alternatives considered**:
- *Permit only the exact host the user typed.* Rejected: breaks the common `www.` ↔ apex and
  `m.`/`api.` subdomain patterns, producing a gate that fails on legitimate use.
- *Use the associated login plugin's declared URL pattern as the anchor.* Rejected as the primary
  source: a URL pattern is a path-level regex in this codebase (`nhentai\.net/g/`), not a host
  declaration, and it is intentionally sometimes narrower than the domain. It would be used only as
  a fallback, and only if a host can be extracted unambiguously.
- *Use the first test link's host as the only anchor.* Rejected: a session may legitimately involve
  more than one of the user's links.

---

## 4. Blocking private and loopback destinations, and the residual risk

**Decision**: Resolve each destination host before connecting and reject if *any* resolved address is
loopback, private, link-local, unique-local, unspecified, or multicast. Pin the resolved addresses for
that request so the connection cannot be redirected to a different address after the check.

**Rationale**: a host-string check alone is defeated by a name that resolves into private space
(`localhost.attacker.example` → `127.0.0.1`), which is precisely the case a model-composed URL can
produce. Pinning closes the window between check and connect. Recording the residual risk matters as
much as closing it: the pin covers the resolution this code performed, and a future change that
bypasses the seam (a new fetcher, a new client builder) reintroduces the gap silently — which is why
the policy lives at the one function every fetch goes through rather than at each call site.

**Alternatives considered**:
- *Check the host string only.* Rejected: defeated by DNS.
- *Rewrite the URL to the resolved IP and set a Host header.* Rejected: breaks TLS SNI/certificate
  validation against the original hostname without additional machinery, and the resolver-pinning
  route achieves the same guarantee inside the HTTP client that is already in use.
- *Rely on the deployment's network policy / container isolation.* Rejected as the only control: the
  product ships as a single process users run on their own machines, where the "protected" network is
  their own machine.

---

## 5. What happens to a redirect that leaves the permitted set

**Decision**: Follow the redirect, but drop every substituted credential header at that hop and
disclose in the tool result that credentials were withheld from hop N onward.

**Rationale**: cross-host redirects are ordinary on the real web (apex↔`www`, http→https, CDN and
login-flow redirects), so refusing the whole chain would break a large share of legitimate pages and
push administrators toward disabling the gate. Dropping credentials closes the leak exactly. The
disclosure is what makes the model able to reason correctly instead of concluding the site requires
authentication when the real cause was this gate.

The disclosure is also a testable contract: it turns a silent security decision into an observable
one, which is what lets SC-001 be asserted rather than assumed.

**Alternatives considered**:
- *Stop the chain at the first hop leaving the set.* Rejected: breaks ordinary redirect shapes and
  yields a worse diagnostic (the model cannot tell "site is broken" from "gate intervened").
- *Allow the credential across the redirect when the destination is also permitted.* That is the
  decided behaviour (both hosts in the set → credential retained). Only leaving the set strips it.

---

## 6. Bounding tool results: prune with a continuation, not summarize

**Decision**: Cap what enters model context per result. When capped, deliver a head portion plus an
explicit truncation disclosure (how much was withheld) and a continuation handle, and let the model
request a further bounded portion. Also cap the read itself, so an enormous response never fully
materializes in memory. Additionally track an accumulated per-generation budget; on exhaustion,
terminate with a specific outcome.

**Rationale**: the purpose is to keep the loop's context and cost bounded, and to keep a single
hostile or merely huge page from consuming the generation's whole allowance. A model-free prune plus
an on-demand continuation keeps the information reachable (the model can always ask for more) at zero
extra model cost. The accumulated budget is separate from the per-result cap precisely because the
failure mode is additive: five individually acceptable results are not acceptable together.

**Alternatives considered**:
- *Summarize the page with a model call before injecting.* Rejected: it is a second billed call whose
  own output must then be gated and bounded, and it destroys verbatim content that generation
  legitimately needs (exact selectors, JSON keys, header names).
- *Hard-truncate with no continuation.* Rejected: silently withholds the part of the page the model
  actually needed and produces a worse failure ("the plugin reads a field that is not there") than an
  explicit "there is more, ask for it".
- *Cap only per result, no accumulated budget.* Rejected: leaves the additive case open.

---

## 7. Repair: exactly one attempt, and only on a validator's verdict

**Decision**: Where a deterministic validator rejects output, issue at most one repair attempt whose
input names the specific defect (the unassigned entries, the index that does not correspond to a
member), then stop. No path retries a validator failure twice. No repair path exists where no
validator does.

**Rationale**: the value is not "retry on failure", it is "retry on *proof*". A deterministic
validator's verdict is a fact, so a repair attempt is well-founded and bounded; a judgment that output
is "poor" is not, and making that a loop produces the classic unbounded self-correction failure this
codebase has no budget to absorb. Naming the defect rather than restating the task is what makes the
single attempt worth its cost.

**Alternatives considered**:
- *Two or three attempts.* Rejected: each additional attempt multiplies cost against an unproven
  premise; one informed attempt captures nearly all of the recoverable value.
- *Feed the validator's raw error text.* Rejected: the two validators' messages are written for
  operators (`LLM split suggestion is incomplete, N file(s) unassigned: first missing = …`), not for
  the model. The repair prompt carries the structured defect instead.

---

## 8. Usage ledger: daily counters, not event rows

**Decision**: Record per-feature counters in Redis, bucketed by day, accumulated with `HINCRBY`-style
increments: request count, elapsed milliseconds, token counts (in/out/total), and per-class failure
counts. Token figures are absent, not zero, when the provider does not report them. Report per-feature
figures over day and week windows from one endpoint.

**Rationale**: the questions to answer are aggregates over a window, and counters answer them in
constant space. The distinction between "no tokens reported" and "zero tokens" is the one that makes
the ledger trustworthy, so it is a property of the shape, not of the formatting.

**Alternatives considered**:
- *Per-call event rows.* Rejected: unbounded growth for a question no one asks per-call, and it would
  duplicate the generation record's job (which does need per-attempt detail, for one feature only).
- *Reuse the translation usage budget ledger.* Rejected: that one exists to *enforce* a spend limit
  with four granularities and a hard stop; the ledger here only *observes*. Merging them would give
  the observe-only path enforcement semantics it must not have (no AI feature may start refusing to
  run because a counter says so).

---

## 9. Generation records: persisted, secret-free, retention-bounded

**Decision**: For the wizard, persist the ordered set of model-visible rounds — the request body
(with credentials represented by their placeholder form, never their values), each round's tool calls
and its bounded tool result — under a per-generation key with a TTL. Failed and retried rounds are
stored as distinct attempts. Recovery must work after a restart.

**Rationale**: the current slot is a single-use in-memory map, so a bug report is unreproducible by
construction. The record makes the one genuinely unanswerable question ("what was the model shown?")
answerable without introducing a general session log — which would be the compaction/session
construct this feature deliberately does not adopt.

Storing the *bounded* tool result rather than the raw page is deliberate: it is exactly what the model
saw, which is what reproducibility needs, and it inherits the context bound rather than duplicating it.

**Alternatives considered**:
- *A general append-only session log for all AI features.* Rejected: nine features, one of which has
  rounds. That is the excluded construct arriving through the side door.
- *Keep it in memory with a larger cap.* Rejected: does not survive the restart that is the whole
  reason the record exists.

---

## 10. Translation is touched at the boundary only

**Decision**: `lanrurugi-translate` gains a mapping from its existing typed error taxonomy into the
new shared vocabulary at the seam boundary, and gains usage attribution. Its internal pipeline,
budget, cache, prefetch and pruning behaviour is not changed.

**Rationale**: translation already implements the constructs this feature adds elsewhere — bounded
usage with enforcement, per-page failure isolation, a four-way typed error taxonomy, and pruning of
what it sends. Rewriting it into the new seam would be a large change with negative expected value,
and its own spec records several deliberate positions (batching granularity, prefix ordering, the
rejection of append-only sessions) that a mechanical unification would quietly violate.

**Alternatives considered**:
- *Make translation the reference implementation of the new seam and migrate it first.* Rejected:
  highest-risk change for lowest incremental benefit, and its error taxonomy is richer in ways the
  shared vocabulary would flatten.

---

## 11. Ordering against in-flight work on the same branch

**Decision**: Implement in the order: outbound fetch policy → LLM seam → ledger/endpoint/panel →
result bounding / bounded repair / record.

**Rationale**: the first two phases touch files no other in-flight work owns, so they can land and be
verified independently; the last two touch the wizard's generation path and the frontend settings
surface, which are the highest-overlap areas. Ordering by overlap rather than by story number means
the security slice is not waiting behind a merge.

**Alternatives considered**:
- *Implement strictly in priority order including frontend.* Rejected: puts the highest-conflict file
  (`generate.rs` + settings UI) in the critical path for no delivery benefit.

---

## 12. Resolved unknowns

| Unknown from Technical Context | Resolution |
| --- | --- |
| Where the credential's permitted host set comes from | Decision 3 — user-supplied domain and link hosts, computed server-side |
| Whether a per-call bound can truncate a legitimate long generation | Resolved: the per-call ceiling sits above the longest observed single round; the wizard's existing overall bound stays as the outer limit |
| Where usage attribution is stored | Decision 8 — new Redis namespace, day-bucketed counters |
| Whether tool results should be summarized or pruned | Decision 6 — model-free prune plus continuation |
| Whether the wizard's browser-driven repair should move server-side | Resolved in part: the *record* moves server-side (Decision 9); the trigger stays where the user's confirm click is, so no autonomous repair loop is introduced |
| Whether any touched endpoint is legacy-shaped | Resolved: none — verified against the route policy file; all are LANrurugi-only additions |
