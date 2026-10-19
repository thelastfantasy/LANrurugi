# Contract: Model Call

**Feature**: `014-ai-harness-hardening` | **Story**: US2 (with US3's measurement hook) | **Requirements**: FR-007…FR-013

## Purpose

One seam through which all 14 server-side model calls pass. Today each call site composes its own
error message from a locally-written status ladder, and 12 of 14 have no bound at all. This contract
replaces that with:

1. one failure vocabulary,
2. one deadline that covers any retry,
3. one retry rule (at most once, transient classes only),
4. one measurement report.

## Failure vocabulary

| Variant | Meaning | Retried? | Reported to the user as |
| --- | --- | --- | --- |
| `NotConfigured` | no API key configured | no | "API key not configured" |
| `Auth` | provider rejected the credential (401) | **no** | key invalid (401) |
| `Balance` | account has no funds / is unavailable (402) | **no** | insufficient balance (402) |
| `RateLimited { retry_after }` | provider throttled (429) | once, clamped to the deadline | rate limited (429) |
| `Transient` | network failure, 5xx, connection reset | once | service temporarily unavailable |
| `Timeout` | the call's own deadline elapsed | no | timed out |
| `Malformed` | unparseable body, missing content, truncated stream | once (truncation only) | response incomplete or unparseable |
| `Rejected { reason }` | a deterministic validator refused the output | **no** (see repair contract) | validator message |

The rendered message for every variant MUST be byte-identical to what that call site produces today,
so adopting the vocabulary changes no user-visible text. The type also converts losslessly into the
string error the existing call sites already return, so adoption is additive and no call site's
control flow changes.

**Invariant**: no call site may construct a generic "AI failed" message when the class is knowable.
A failure whose class is unknown is a seam bug, not a call-site decision.

## Deadline

- Every call carries a deadline; the deadline covers all attempts of that call.
- The default per-call ceiling is set above the longest observed legitimate single round (the
  long-form generation round observed at 80+ seconds) and remains below the wizard's existing overall
  bound, which stays as the outer limit for multi-round actions.
- A `retry_after` hint is honoured only if it fits inside the remaining deadline; otherwise the call
  fails immediately as `RateLimited` rather than sleeping past its own bound.
- No default transport timeout is relied upon. A call with no explicitly supplied deadline receives
  the seam's own default.

## Retry rule

- At most **one** retry, and only for `Transient`, `RateLimited`, and `Malformed`-by-truncation.
- `Auth`, `Balance`, `NotConfigured` and `Timeout` are never retried — retrying them spends the
  user's time against a condition that cannot change within the call.
- The existing behaviour of retrying a truncated stream with reduced reasoning effort is preserved;
  it is expressed as this rule's `Malformed`-by-truncation case rather than as a separate special
  path.
- A retry never extends the action's overall bound.

## Measurement

Every call yields a report even when it fails:

| Field | Notes |
| --- | --- |
| `feature` | the closed-set id supplied by the caller |
| `outcome` | the vocabulary variant, including success |
| `attempts` | 1 or 2 |
| `elapsed_ms` | across all attempts |
| `prompt_tokens`, `completion_tokens`, `total_tokens` | `None` when the provider did not report usage; never `0` as a stand-in |

The caller MUST pass its feature id. The seam does not infer it — inference would silently misattribute
once a caller is refactored.

## Relationship to the repair contract

`Rejected` exists so that a deterministic validator's refusal is a first-class outcome rather than a
formatted string. Whether a repair attempt is issued is decided by the repair contract, not here; the
seam only guarantees that a `Rejected` outcome is never retried internally, which is what makes the
"at most one repair" bound enforceable.

## Non-goals

- Not a provider abstraction for its own sake. Adding a second provider is authorised by this
  contract's shape but is not delivered here.
- Not a circuit breaker, a global concurrency limiter, or a cost governor. The translation pipeline's
  existing spend enforcement is unchanged and is not generalised by this contract.
