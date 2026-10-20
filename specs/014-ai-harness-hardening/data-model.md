# Phase 1 Data Model: Agent-Harness Hardening

**Feature**: `014-ai-harness-hardening`

Two of the five entities below are ephemeral (computed per call, never stored) and three are
persisted. Persisted entities use the repository's existing additive-key convention: a
`LANRURUGI_` prefix, one key per bucket where the bucket is naturally time-delimited, and per-key
TTL rather than a sweep. No existing key is read differently or rewritten.

---

## 0. Feature ids (closed set)

`FeatureId` is a newtype over a `&'static str`, supplied by each call site; the seam never infers it.
The set is closed and returned by the usage endpoint in this order:

| Id | Call sites |
| --- | --- |
| `plugin_generation` | `plugin_wizard/generate.rs` (1) |
| `plugin_login_analysis` | `plugin_wizard/analyze_login.rs` (2, including the JSON-reformat round) |
| `plugin_trial_attribution` | `plugin_wizard/trial_run.rs` (1) |
| `archive_split` | `archive_split.rs` (1) |
| `subfolder_tankoubon_naming` | `scripts.rs` (1) |
| `artist_backfill` | `artist_backfill.rs` (3) |
| `tankoubon_rename` | `tankoubons.rs` (2) |
| `recommend_rerank` | `recommend_llm.rs` (1) |
| `subscription_condition` | `subscriptions/ai_condition.rs` (1) |
| `provider_pricing` | `lanrurugi-translate/src/pricing.rs` (1) |
| `translation` | the translation pipeline, attributed at its own seam boundary (T043) |

The first ten are the 14 server-side calls; `translation` is the eleventh feature in the ledger.

---

## 1. Model Call (ephemeral)

One request to a model provider. Produced by the call seam, consumed by the caller and by the ledger.

| Field | Type | Notes |
| --- | --- | --- |
| `feature` | `FeatureId` (§0) | Attributed by the caller; the seam never guesses |
| `outcome` | one of `ok`, `not_configured`, `auth`, `balance`, `rate_limited`, `transient`, `timeout`, `malformed`, `rejected`, `cancelled` | The single failure vocabulary; `cancelled` is not a failure and is never retried |
| `attempts` | integer, 1 or 2 | 2 only when a transient class was retried once |
| `elapsed_ms` | integer | Wall clock for the whole call including its retry |
| `prompt_tokens` / `completion_tokens` / `total_tokens` | optional integers | `None` means *the provider did not report* — never zero |
| `prompt_cache_hit_tokens` / `prompt_cache_miss_tokens` | optional integers | DeepSeek's split of `prompt_tokens`; priced differently, so cost cannot be derived from `prompt_tokens` alone |
| `reasoning_tokens` | optional integer | From `completion_tokens_details`, when reported |
| `retry_after` | optional duration | Carried through from a rate-limit response; clamped to the remaining deadline |

**Validation rules**

- `outcome == ok` implies at least one provider response was received.
- `attempts == 2` implies the first attempt's class was `transient` or `malformed`-by-truncation;
  no other class retries (FR-008/FR-009).
- `prompt_tokens` present implies the provider returned a usage block; the three token fields are
  never partially fabricated.
- `elapsed_ms` covers all attempts, so a retried call cannot appear faster than its bound.

---

## 2. Attributed Usage (persisted)

Per-feature, per-day counters. Questions answered: "requests, tokens and elapsed time for feature X
over window W".

**Key layout**

| Key | Type | TTL | Contents |
| --- | --- | --- | --- |
| `LANRURUGI_LLM_USAGE:<feature>:<YYYY-MM-DD>` | hash | 40 days | `requests`, `elapsed_ms`, `prompt_tokens`, `completion_tokens`, `total_tokens`, `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens`, `reasoning_tokens`, `unreported_token_calls`, `cancelled`, and one counter per failure class (`fail_auth`, `fail_balance`, `fail_rate_limited`, `fail_transient`, `fail_timeout`, `fail_malformed`, `fail_rejected`) |

Dated keys rather than one hash, for the same reason the existing read-daily counter uses dated
keys: retention becomes each key's own TTL, so there is no pruning code and no unbounded hash.

**Derived view** (`GET /api/llm/usage`): for a requested window, sum the day keys per feature and
report, per feature, `requests`, `elapsed_ms`, each token figure with a presence flag, and the
failure breakdown. A feature with no keys in the window reports `no_data`, never a zero-filled row —
that distinction is what keeps the panel honest about a feature nobody has used.

**Validation rules**

- A counter is only ever incremented; no read-modify-write of a whole hash, so concurrent writers
  cannot lose each other's increments.
- `unreported_token_calls` counts calls in the window whose token figures were absent, so a
  consumer can tell "cheap" from "unmeasured".
- Summing a window that spans a missing day yields the days that exist; no synthetic zero days are
  materialized.

---

## 3. Fetch Policy and Tool-Request Decision (ephemeral)

Computed once per generation, evaluated on every outbound request.

**Fetch Policy**

| Field | Type | Notes |
| --- | --- | --- |
| `fetch_hosts` | set of hosts | The wizard's target domain plus the hosts of user-supplied test links and reference URLs. A request to any other host is denied |
| `credential_hosts` | set of hosts | The target domain only. A credentialed request, or a hop that keeps credentials, must stay inside it (FR-002) |
| `credential_fields` | set of field names | The names that may appear as a placeholder; used to decide whether a request is *credentialed* |

Both host sets use label-boundary matching: a proper subdomain of a member is a member; ports are
ignored.

**Tool-Request Decision** — the outcome of evaluating one request.

| Field | Type | Notes |
| --- | --- | --- |
| `verdict` | `allow` \| `deny` \| `allow_without_credentials` | `allow_without_credentials` is a redirect hop that leaves `credential_hosts` but stays inside `fetch_hosts` |
| `reason` | one of `scheme_not_allowed`, `host_not_permitted`, `credential_host_not_permitted`, `private_address`, `unresolvable_host`, `unknown_credential_reference`, `redirect_left_permitted_set` | Machine-readable; each maps to a model-legible explanation. `redirect_left_permitted_set` ends the chain at a hop outside `fetch_hosts` |
| `withheld_at_hop` | optional integer | Set only for `allow_without_credentials` |
| `resolved_addresses` | optional list | Retained for diagnostics; never exposed to the model |

**Validation rules (the gate's invariants)**

- A request carrying a substituted credential must have `verdict == allow`. There is no code path
  that substitutes first and evaluates later (FR-001/FR-002).
- `deny` is the default for any input the evaluation cannot classify — including an unparseable URL,
  an unreadable host, and an unresolvable name (FR-005).
- Every redirect hop is evaluated independently; the decision recorded in the result is the
  strictest verdict any hop produced (FR-003).
- A resolved address in loopback, private, link-local, unique-local, unspecified or multicast space
  forces `deny` regardless of host permission (FR-004).
- A credential placeholder with no corresponding field in `credential_fields` is `deny`, never an
  empty substitution and never a literal pass-through (FR-006).
- A credentialed request whose host is outside `credential_hosts` is `deny /
  credential_host_not_permitted`, even when the host is in `fetch_hosts` (FR-002).
- A redirect hop outside `fetch_hosts` ends the chain with `deny / redirect_left_permitted_set`;
  nothing is sent to that hop's destination (FR-003).
- Both host sets are derived exclusively from values the user typed in the request; no model output
  can widen them.

**Trial-Run Host Check** (ephemeral, FR-028) — evaluated before a trial run executes a draft.

| Field | Type | Notes |
| --- | --- | --- |
| `declared_hosts` | set of hosts | From the draft's own `pluginInfo()` network permissions |
| `outside_hosts` | set of hosts | `declared_hosts` not inside `fetch_hosts` |
| `approved_hosts` | set of hosts | Sent by the frontend from an explicit user confirmation for this one run; never remembered |
| `verdict` | `run` \| `needs_approval` | `run` iff `outside_hosts ⊆ approved_hosts` |

The draft runs with exactly its declared hosts; this check only decides whether it may run at all.
The host-side plugin HTTP client's existing per-hop enforcement of declared hosts is unchanged.

**State transitions** — a request moves `proposed → evaluated → (executed | refused)` exactly once.
An evaluated request is never re-evaluated with a widened policy within the same generation.

---

## 4. Bounded Tool Result (ephemeral)

| Field | Type | Notes |
| --- | --- | --- |
| `status` | `ok` \| `cf_challenge` \| `truncated` \| `redirect_cap_exceeded` \| `refused` \| `error` | Extends today's set with `truncated` and `refused` |
| `content` | string | Head portion, bounded |
| `total_bytes` | integer | Size of what the source actually returned, so the disclosure is quantitative |
| `returned_bytes` | integer | Size of `content` |
| `continuation` | optional string | Handle for requesting the next bounded portion |
| `withheld_credentials_from_hop` | optional integer | Present only for the redirect case |
| `refusal_reason` | optional string | Present only for `refused`; equals the decision's `reason` |

**Validation rules**

- `returned_bytes <= total_bytes` always; `status == truncated` iff `returned_bytes < total_bytes`.
- A `refused` result never carries `content` — a refusal must not leak the response of a request that
  was denied.
- `content` reaches the model inside an untrusted-data frame (FR-030); a result under the cap is
  otherwise byte-identical to today's (FR-019).
- A continuation handle addresses a specific source and byte range; it must not be usable to reach a
  different host than the one it was issued for (it is re-evaluated like any other request).
- The read itself is capped, so `total_bytes` for an enormous response is an upper bound rather than
  a full materialization in memory.

---

## 5. Attempt and Generation Record (persisted)

An **Attempt** is one model-visible round. A **Generation Record** is the ordered set of Attempts for
one generation action, identified by a `GenerationId` — a `#[serde(transparent)]` newtype over the
existing generation id string (constitution: domain identifiers are newtypes from first
implementation). The pending-request map and the record key builder both take `GenerationId`.

**Header** (index 0, kind `request`): the generation request with credential fields reduced to
placeholder form, plus the `model`, the request parameters actually sent (reasoning effort,
temperature, max tokens) and the system prompt.

**Attempt**

| Field | Type | Notes |
| --- | --- | --- |
| `index` | integer | Order within the generation |
| `kind` | `request` \| `tool_round` \| `failed_attempt` \| `trial_run` | A failed or retried round is its own kind, never mistaken for content the model was shown (FR-025); `trial_run` holds a trial-run result and its attribution (FR-022) |
| `messages` | list | The messages actually sent for this round, credential values in placeholder form; omitted for `trial_run` |
| `tool_calls` | list of `{name, arguments}` | Arguments are stored in placeholder form for credentialed headers |
| `tool_result` | optional bounded tool result | Exactly what the model saw, hence already bounded |
| `outcome` | one of the model-call outcomes | Failure classes recorded, not elided; `cancelled` when FR-029 stopped the round |
| `trial` | optional `{plugin_type, ok, error, attribution}` | Only for `trial_run`; `attribution` is the trial-attribution call's verdict |

**Record key**: `LANRURUGI_GENERATION_RECORD:<generation_id>` — a list, TTL 30 days.

**Validation rules**

- No cleartext credential value appears in any persisted field (FR-026). The single place a value is
  substituted is the outbound request itself; the record stores the pre-substitution form.
- The record is written before the round's request is issued, so a crash mid-round leaves the
  intent rather than losing it.
- A generation with zero model calls has no record.
- Expiry is per-key TTL; expiry of a record affects nothing else, and no other feature's data shares
  the namespace (FR-027).
- The record is readable only by the same session-scoped authority the wizard endpoints already use.

---

## Entity relationships

```text
Generation action (1) ──< Attempt (n) ──< Tool-Request Decision (per tool call)
        │
        └──< Model Call (n)  ──> Attributed Usage (per feature, per day, aggregated)
```

- One generation action produces many attempts; each attempt may contain tool calls; each tool call
  has one decision and at most one bounded result.
- Each model call — including repair attempts at non-wizard call sites — contributes exactly one
  increment set to Attributed Usage.
- Attributed Usage aggregates across features and generations and is deliberately *not* joinable
  back to individual records; the two answer different questions (spend vs. reproduction).
