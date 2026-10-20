# Quickstart: Agent-Harness Hardening

**Feature**: `014-ai-harness-hardening`

Each scenario below is a runnable validation of one user story's Independent Test. They are ordered
by story; none depends on another, and each states what it proves in one line.

## Prerequisites

| Need | How |
| --- | --- |
| Rust toolchain in the dev image | `mise run dev-build` once, then everything runs through `mise run` (never bare `cargo`) |
| Redis for the persisted-record scenarios (S3, S6) | `mise run dev-up`; tests skip themselves when `LANRURUGI_TEST_REDIS_URL` is unset |
| A provider that can be made to misbehave | a local stub endpoint reached by `LANRURUGI_DEEPSEEK_BASE_URL`; already supported and used today, so no new test seam is needed |
| A real provider key | only for the optional end-to-end pass; the scripted scenarios do not need one |

> Scenario tests run per crate (`mise run test-crate -- <crate>`). The full `mise run test` is run by
> the user, not by the implementing agent, because of host memory pressure; the agent reads its log.

## S1 — A credential never leaves its own site

**Proves**: US1 / FR-001…FR-006, FR-028, FR-030 / SC-001. This is the security scenario; run it first.

Setup: a Redis-backed test state with a login plugin whose declared field is `token`, a wizard
generation request carrying `target_domain = "site-a.example"`, test links on `site-a.example` and
on `site-c.example` (a user-supplied second site), and `credential_values = { "token":
"SECRET-<random>" }`. A local stub server stands in for each host; `site-b.example` is never
supplied by the user.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Model requests `https://site-a.example/docs` with `Authorization: Key {{token}}` | request reaches the stub with the real value substituted; result `status: "ok"` |
| 2 | Model requests `https://site-b.example/` with the same header | **no request reaches site B's stub at all**; result `status: "refused"`, `refusal_reason: "host_not_permitted"` |
| 3 | Model requests `https://site-a.example/start` which 302s to `https://site-b.example/x` | the chain ends at that hop: result `status: "refused"`, `refusal_reason: "redirect_left_permitted_set"`, naming `site-b.example`; **site B's stub receives no request at all** |
| 3b | Model requests `https://site-a.example/cdn` which 302s to `https://site-c.example/img` | followed **without** the credential header; result carries `withheld_credentials_from_hop: 1`; site C's stub records no `Authorization` header |
| 4 | Model requests `https://img.site-a.example/p.png` (subdomain) | allowed — label-boundary subdomain rule |
| 5 | Model requests `https://not-site-a.example/` | refused (`evil-` style suffix is not a subdomain) |
| 6 | Model requests `http://127.0.0.1:8500/`, `http://169.254.169.254/`, `file:///etc/passwd` | each refused: `private_address`, `private_address`, `scheme_not_allowed`; nothing is dialled |
| 7 | Model sends `Authorization: Key {{nonexistent}}` | refused `unknown_credential_reference` — the header is **not** sent with `{{nonexistent}}` literal, and **not** emptied |
| 8 | Model requests `https://site-a.example/` with no credential reference | allowed, and the request is byte-identical to today's behaviour |
| 9 | Model requests `https://site-c.example/` with `Authorization: Key {{token}}` | refused `credential_host_not_permitted`; site C's stub receives nothing |
| 10 | Model requests `https://site-c.example/` with no credential reference | allowed |
| 11 | Trial run of a draft declaring network hosts `site-a.example`, `evil.example` | refused `409 trial_hosts_need_approval` listing `evil.example`; **nothing executes** (no Deno process, no request at any stub) |
| 12 | Same trial run with `approved_hosts: ["evil.example"]` | runs as today |
| 13 | Inspect the model input after any successful fetch | the page content sits inside the untrusted-data frame, and the system prompt carries the rule about instructions inside it |

Assertion that must be mechanical, not visual: the stub servers record every inbound request; steps
2, 3, 6, 9 and 11 assert **zero** inbound requests at the foreign stub, and step 3b asserts the
header present on hop 1 and absent at site C.

## S2 — No AI action can hang, and every failure has a name

**Proves**: US2 / FR-007…FR-011, FR-029, FR-031 / SC-002, SC-003, SC-009.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Point the provider at a stub that accepts the connection and never writes | every AI action terminates with the `timeout` class by its documented bound; the wizard's overall bound still dominates for multi-round actions |
| 2 | Stub returns `500` once, then succeeds | exactly 2 attempts; result succeeds; `attempts == 2` |
| 3 | Stub returns `401` | 1 attempt, `Auth`, no retry |
| 4 | Stub returns `402` | 1 attempt, `Balance`, no retry |
| 5 | Stub returns `429` with `Retry-After: 1` | retried once after the hint; if the hint exceeds the remaining deadline, fails immediately as `RateLimited` |
| 6 | Stub returns a truncated stream (no `finish_reason`) | retried once with reduced reasoning effort, exactly as today, expressed as the `Malformed`-by-truncation case |
| 7 | Enumerate all failure paths in the suite | every one maps to a named class; no path yields a generic message |
| 8 | Start a generation against a slow stub, then drop the event-stream receiver mid-round | the in-flight provider request is dropped, no further request reaches the stub, the generation ends within 5 s, outcome `cancelled` |
| 9 | Truncated stream after progress events were already emitted (step 6's case mid-stream) | a `retrying` event precedes the retried round's output; the record holds the abandoned round as `failed_attempt` |

Run across all 10 server-side features, not just the wizard — the point of the story is that the 12
previously unbounded call sites are now bounded.

## S3 — Usage is attributable per feature

**Proves**: US3 / FR-012…FR-015 / SC-004.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Exercise three different AI features (any outcome) | `GET /api/llm/usage?window=day` reports `reported` for exactly those three, `no_data` for the other eight |
| 1b | Stub reports `prompt_cache_hit_tokens`, `prompt_cache_miss_tokens` and reasoning tokens | the three figures appear separately; a feature whose stub omitted them shows them absent, not `0` |
| 1c | A cancelled generation | counted under `cancelled`, not under any failure class |
| 2 | Inspect a feature whose stub returned no usage block | its token figures are **absent**, `unreported_calls > 0`; the panel renders "not reported", not `0` |
| 3 | Restart the server, re-query the week window | the same figures are still reported (persisted, not in-process) |
| 4 | Query as a guest | denied by the route policy before the handler |
| 5 | Query with `window=bogus` | `400 invalid_window` |

## S4 — A huge page cannot consume the generation

**Proves**: US4 / FR-016…FR-019 / SC-005.

Setup: a stub serving a page an order of magnitude larger than a typical plugin-authoring page.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Model fetches the huge page | result `status: "truncated"`, `returned_bytes` bounded, `total_bytes` > `returned_bytes`, `continuation` present; the model-visible content is bounded |
| 2 | Model uses the continuation | a further bounded portion arrives; the request is re-evaluated by the fetch policy like any other |
| 3 | Five large pages in one generation | the generation's accumulated context stays within budget and the action terminates with either code or a specific outcome — never an unbounded request |
| 4 | An ordinary page (well under the cap) | delivered unchanged; `status: "ok"`, no continuation, byte-identical to today |

## S5 — One informed repair, never a loop

**Proves**: US5 / FR-020…FR-023 / SC-006.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Split-suggestion stub returns a grouping that leaves files unassigned | exactly one repair attempt, whose input names the unassigned entries; if the second output covers everything, the action succeeds |
| 2 | Second output also leaves files unassigned | the action fails after exactly one attempt; no third request is issued (assert by request count) |
| 3 | Rename stub returns a chapter list whose indices do not correspond to members | refused by the deterministic validator, one repair attempt, then stop |
| 4 | Rename stub returns a valid-but-poor list | no repair attempt — no validator rejected it |
| 5 | Any call site without a deterministic validator | no repair path is invoked, regardless of output quality |

## S6 — A generation is reconstructible after a restart

**Proves**: US6 / FR-024…FR-027 / SC-007.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Run a generation with credentials entered, including at least one refused fetch | `GET /api/plugin-wizard/generate/{id}/record` returns attempts in order |
| 2 | Restart the server, re-query | identical record |
| 3 | Grep the record for the known credential value | **zero occurrences**; only placeholder forms appear |
| 4 | Inspect a round that failed and was retried | it appears with `kind: "failed_attempt"`, distinguishable from rounds the model was shown |
| 4b | Inspect the header and any round | the header carries the model, request parameters and system prompt; each round carries the messages as sent, credentials in placeholder form |
| 4c | Run a trial run for the generated draft, then re-read the record | a `trial_run` attempt with the result and its attribution |
| 5 | Query a random id | `404` |
| 6 | Query as a guest | denied by route policy |

The credential scan in step 3 is the assertion that matters: it must run as a test over a record
produced from a real generation, not as a manual read.

## S7 — Nothing else changed

**Proves**: SC-008.

- `mise run test` (run by the user) passes unchanged for every existing AI feature's success-path
  tests.
- The same generation inputs produce byte-identical output for the same stub responses (recording the
  request bodies the stub receives before and after, and diffing them) — the only permitted difference
  being an added bounded-result field on truncated/refused results.
- The usage panel and the trial-run host approval are handed to the user to confirm in the running
  app; neither ports legacy UI, and the repository's rule is that visual sign-off is the user's.

## Gates before handing off

```sh
mise run check-crate -- lanrurugi-llm lanrurugi-api
mise run fmt-check
mise run check        # rust-check + frontend-lint, per the pre-push rule
```

`mise run test` is the full gate; the user runs it (see Prerequisites).
