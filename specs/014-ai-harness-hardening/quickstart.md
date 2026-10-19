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

> Long test runs must be launched detached from the agent/editor process tree, per this repository's
> own rule: `setsid nohup mise run test > /tmp/test-output.log 2>&1 < /dev/null &`. Do not use the
> harness's own background mechanism for `mise run test`.

## S1 — A credential never leaves its own site

**Proves**: US1 / FR-001…FR-006 / SC-001. This is the security scenario; run it first.

Setup: a Redis-backed test state with a login plugin whose declared field is `token`, a wizard
generation request carrying `target_domain = "site-a.example"`, a test link on `site-a.example`, and
`credential_values = { "token": "SECRET-<random>" }`. A local stub server stands in for each host.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Model requests `https://site-a.example/docs` with `Authorization: Key {{token}}` | request reaches the stub with the real value substituted; result `status: "ok"` |
| 2 | Model requests `https://site-b.example/` with the same header | **no request reaches site B's stub at all**; result `status: "refused"`, `refusal_reason: "host_not_permitted"` |
| 3 | Model requests `https://site-a.example/start` which 302s to `https://site-b.example/x` | the redirect is followed **without** the credential header; result carries `withheld_credentials_from_hop: 1`; site B's stub records no `Authorization` header |
| 4 | Model requests `https://img.site-a.example/p.png` (subdomain) | allowed — label-boundary subdomain rule |
| 5 | Model requests `https://not-site-a.example/` | refused (`evil-` style suffix is not a subdomain) |
| 6 | Model requests `http://127.0.0.1:8500/`, `http://169.254.169.254/`, `file:///etc/passwd` | each refused: `private_address`, `private_address`, `scheme_not_allowed`; nothing is dialled |
| 7 | Model sends `Authorization: Key {{nonexistent}}` | refused `unknown_credential_reference` — the header is **not** sent with `{{nonexistent}}` literal, and **not** emptied |
| 8 | Model requests `https://site-a.example/` with no credential reference | allowed, and the request is byte-identical to today's behaviour |

Assertion that must be mechanical, not visual: the stub servers record every inbound request; step 2
and step 6 assert **zero** inbound requests, and step 3 asserts headers present on hop 1 absent on
hop 3.

## S2 — No AI action can hang, and every failure has a name

**Proves**: US2 / FR-007…FR-011 / SC-002, SC-003.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Point the provider at a stub that accepts the connection and never writes | every AI action terminates with the `timeout` class by its documented bound; the wizard's overall bound still dominates for multi-round actions |
| 2 | Stub returns `500` once, then succeeds | exactly 2 attempts; result succeeds; `attempts == 2` |
| 3 | Stub returns `401` | 1 attempt, `Auth`, no retry |
| 4 | Stub returns `402` | 1 attempt, `Balance`, no retry |
| 5 | Stub returns `429` with `Retry-After: 1` | retried once after the hint; if the hint exceeds the remaining deadline, fails immediately as `RateLimited` |
| 6 | Stub returns a truncated stream (no `finish_reason`) | retried once with reduced reasoning effort, exactly as today, expressed as the `Malformed`-by-truncation case |
| 7 | Enumerate all failure paths in the suite | every one maps to a named class; no path yields a generic message |

Run across all 9 features, not just the wizard — the point of the story is that the 12 previously
unbounded call sites are now bounded.

## S3 — Usage is attributable per feature

**Proves**: US3 / FR-012…FR-015 / SC-004.

| Step | Action | Expected |
| --- | --- | --- |
| 1 | Exercise three different AI features (any outcome) | `GET /api/llm/usage?window=day` reports `reported` for exactly those three, `no_data` for the other six |
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
| 5 | Query a random id | `404` |
| 6 | Query as a guest | denied by route policy |

The credential scan in step 3 is the assertion that matters: it must run as a test over a record
produced from a real generation, not as a manual read.

## S7 — Nothing else changed

**Proves**: SC-008.

- `mise run test` passes unchanged for every existing AI feature's success-path tests.
- The same generation inputs produce byte-identical output for the same stub responses (recording the
  request bodies the stub receives before and after, and diffing them) — the only permitted difference
  being an added bounded-result field on truncated/refused results.
- The new settings panel's computed style matches the settings surface it sits in across all 5 themes
  — the repository's mandatory UI verification rule: `font-size`, `font-weight`, `line-height`,
  rendered width/height, `border-radius`, `padding`, `margin`, pulled from `getComputedStyle` on both
  sides rather than compared by screenshot.

## Gates before handing off

```sh
mise run check-crate -- lanrurugi-llm lanrurugi-api
mise run fmt-check
mise run check        # rust-check + frontend-lint, per the pre-push rule
```

`mise run test` is the full gate and must be launched detached (see Prerequisites).
