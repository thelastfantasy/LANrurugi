---

description: "Task list for feature 014-ai-harness-hardening"
---

# Tasks: Agent-Harness Hardening for LANrurugi's AI Call Sites

**Input**: Design documents from `/specs/014-ai-harness-hardening/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/

**Tests**: Included, and only where a measurable outcome makes them mandatory rather than optional.
SC-001 requires that "100% of requests to a foreign host are refused **before any traffic leaves the
server**" — that is only checkable by a stub server asserting zero inbound requests. SC-007 requires
that a recovered record contains **no cleartext credential**, which the generation-record contract
states must be "asserted by a scan test, not by review". SC-008 requires that ordinary-path behaviour
does not change, which needs a byte-diff of the requests the provider stub receives, captured before
the refactor. Where a task has no such obligation it is implementation-only, with no test task added
for its own sake.

**Organization**: Grouped by user story, in priority order. US1 (the security story) is first and is
deliverable alone as an MVP; US2 is next; US3 depends on US2's measurement existing but not on its
retry policy being tuned; US4, US5 and US6 are mutually independent.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies on incomplete tasks)
- **[Story]**: Which user story this task belongs to
- Exact file paths are given in every task

## Path Conventions

Existing Cargo workspace under `crates/` and the single frontend app under `apps/frontend/src/`. No
new crate, no new workspace member (plan.md, Structure Decision).

---

## Phase 1: Setup

**Purpose**: Establish the two ground truths the rest of the feature is measured against. Both must be
captured *before* any refactor, or the SC-008 and error-message guarantees become unfalsifiable.

- [ ] T001 Capture today's exact user-visible AI error strings. Read the two status-code ladders in `crates/lanrurugi-llm/src/lib.rs` (`post_chat_completion` and `tool_chat_streaming_once`) and record every distinct rendered string — 401/402/429/500/503/other, the "key not configured" case, the truncation case, and `json_chat`'s unparseable case — into a documented table plus a Rust test `crates/lanrurugi-llm/tests/error_message_baseline.rs` that asserts the *new* vocabulary's `Display` reproduces each one byte-for-byte. Written as failing-by-construction at first (the vocabulary does not exist yet); it becomes the SC-008 guard once T004 lands.

- [ ] T002 [P] Audit that every endpoint this feature touches is a LANrurugi-only addition with no legacy counterpart, since Principle II constrains changes to *existing* legacy-shaped endpoints only. Check `crates/lanrurugi-api/policy/route_policy.csv` and the legacy route table for: `/api/plugin-wizard/*`, `/api/tankoubons/{id}/ai/rename-suggestions`, `/api/tankoubons/{id}/ai/rename-chapter`, `/api/subscriptions/condition/ai`, `/api/archives/{id}/split/*`, `/api/llm/usage` (new). Record the result in `specs/014-ai-harness-hardening/research.md` §12; if any turns out to be legacy-shaped, stop and amend the plan before writing code.

- [ ] T003 Confirm the baseline is green so later failures are attributable: run `mise run check-crate -- lanrurugi-llm lanrurugi-api lanrurugi-storage` and `mise run fmt-check`.

**Checkpoint**: the error-string table exists, the endpoint audit is recorded, and the touched crates check clean.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: The shared vocabulary, measurement type and key namespaces that US2, US3, US4, US5 and
US6 all build on. US1 does not depend on this phase and may start immediately after Phase 1.

**⚠️ CRITICAL**: US2…US6 cannot begin until this phase is complete.

- [ ] T004 [P] Create `crates/lanrurugi-llm/src/error.rs` implementing contracts/llm-call-contract.md's failure vocabulary: variants for not-configured, auth, balance, rate-limited (carrying an optional retry-after), transient, timeout, malformed, and rejected-by-validator. `Display` must reproduce T001's table exactly. Provide a lossless conversion into `String` so the fourteen existing call sites that return `Result<_, String>` keep compiling with no control-flow change. Add `is_retryable()` per the contract's retry column.

- [ ] T005 [P] Create `crates/lanrurugi-llm/src/usage.rs`: a `FeatureId` newtype over a static string (the seam must not infer which feature called it — inference misattributes the moment a caller is refactored), provider-usage extraction from a parsed chat-completions response body, and a `ModelCallReport` carrying feature, outcome, attempts, elapsed, and the three optional token figures with "absent ≠ zero" (data-model.md §1).

- [ ] T006 [P] Add the two key builders to `crates/lanrurugi-storage/src/keys.rs`, following that file's existing convention for additive keys (`LANRURUGI_` prefix, documented as having no legacy counterpart, dated bucket where the bucket is time-delimited): `llm_usage_daily_key(feature, date)` and `generation_record_key(generation_id)`. Include the TTL figures (40 days, 30 days) as documented constants with their rationale.

- [ ] T007 Replace both duplicated status-code ladders with the T004 vocabulary, keeping every rendered string identical. Both live in `crates/lanrurugi-llm/src/lib.rs`; this is the change that makes the class available to the retry policy and the ledger instead of only to a formatted message.

- [ ] T008 Wire `error.rs`/`usage.rs` into `crates/lanrurugi-llm/src/lib.rs`'s public surface and make the T001 baseline test pass.

**Checkpoint**: `mise run check-crate -- lanrurugi-llm lanrurugi-storage` passes and the error-string baseline test is green.

---

## Phase 3: US1 — A credential is only ever sent to the site it belongs to (P1)

**Goal**: the outbound fetch gate. Everything the wizard fetches on the model's behalf is evaluated
before execution; a substituted credential never leaves its own site; denials are legible to the
model.

**Independent Test**: quickstart S1's eight-step matrix, including the two zero-inbound-request
assertions.

- [ ] T009 [P] [US1] Implement permitted-set derivation in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs`: from the request's target domain plus the hosts of the user-supplied test links and auxiliary reference URLs (contracts/outbound-fetch-policy.md §Inputs). Nothing derived from model output may enter the set — this is the gate's central invariant.

- [ ] T010 [P] [US1] Implement host matching in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs`: DNS-label-boundary comparison plus proper-subdomain allowance. Unit tests must include the `evil-example.com` vs `example.com` case and the `img.example.com` case — a naive suffix match is the single most likely hand-rolled mistake here.

- [ ] T011 [US1] Implement the address-class check and resolution pinning (`crates/lanrurugi-api/src/plugin_wizard/fetch.rs`): resolve before connecting, deny if any resolved address is loopback, private, link-local, unique-local, unspecified or multicast, and pin the resolved addresses for that request so the connection cannot land elsewhere after the check. Document the residual risk (a future second fetcher would bypass the gate) in the function's own doc comment.

- [ ] T012 [US1] Implement the decision procedure itself in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs` and make `fetch_with_redirect_trail` consult it before building any request. Evaluation strictly precedes substitution and traffic — no code path may substitute first (contracts/outbound-fetch-policy.md §Decision procedure).

- [ ] T013 [US1] Extend the redirect policy in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs` to evaluate every hop: a hop that leaves the permitted set continues without credential headers and records `withheld_at_hop`; a hop resolving into private space terminates the chain. Retain the existing 10-hop cap and per-fetch timeout unchanged.

- [ ] T014 [P] [US1] Add the `target_domain` field to the generation request in `crates/lanrurugi-api/src/plugin_wizard/generate.rs` (serde-defaulted so an older client cannot 400), and thread it into policy construction at the call site that currently passes `credential_values` into `run_generation`.

- [ ] T015 [P] [US1] Send `target_domain` from the frontend, from the wizard session's already-existing `domain` state: `apps/frontend/src/pages/PluginWizard/GenerationStep.tsx`, `apps/frontend/src/pages/PluginWizard/RefinePanel.tsx`, `apps/frontend/src/pages/PluginWizard/TrialRunResult.tsx`. All three build the request body before calling `streamGenerate` (`apps/frontend/src/pages/PluginWizard/generateStream.ts`), which itself needs no change.

- [ ] T016 [US1] Surface refusals in `crates/lanrurugi-api/src/plugin_wizard/tool_loop.rs::execute_fetch_tool`: add `status: "refused"` with `refusal_reason`, and a `withheld_credentials_from_hop` field for the redirect case. The refusal text must name the rule that refused it and what would have been permitted, in the model's working language — it must not echo the credential value or the resolved address (contracts/outbound-fetch-policy.md §Result shape).

- [ ] T017 [US1] Add the quickstart S1 matrix as tests in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs`'s test module, using the file's existing real-`127.0.0.1` stub-server pattern for each hop. The foreign-host cases must assert **zero** inbound requests at the foreign stub, and the redirect case must assert the `Authorization` header is present on hop 1 and absent at the destination.

**Checkpoint**: quickstart S1 passes end to end; a same-site credentialed fetch is byte-identical to today's behaviour.

---

## Phase 4: US2 — No AI feature can hang or fail ambiguously (P2)

**Goal**: every model call is bounded, retried at most once and only for transient classes, and reports
one of a closed set of outcomes.

**Independent Test**: quickstart S2's seven-step matrix across all nine features.

- [ ] T018 [P] [US2] Create `crates/lanrurugi-llm/src/retry.rs`: a deadline type that covers all attempts of one call, and the retry decision (at most one; classes per T004's `is_retryable`). The default per-call ceiling must sit above the longest observed legitimate single round (the long-form generation round observed at 80+ seconds) and stay below the wizard's existing 300s overall bound, which remains the outer limit.

- [ ] T019 [US2] Implement the retry execution in `crates/lanrurugi-llm/src/retry.rs`: honour a rate-limit hint only within the remaining deadline, clamp rather than sleep past the bound, and fold the existing "retry with reasoning disabled after a truncated stream" behaviour into the malformed-by-truncation case instead of leaving it as a separate special path in `crates/lanrurugi-llm/src/lib.rs`.

- [ ] T020 [US2] Route all fourteen call sites through one entry point in the new `crates/lanrurugi-api/src/llm_call.rs`, each passing its `FeatureId` and its deadline. The sites, verbatim: `crates/lanrurugi-api/src/scripts.rs` (1), `crates/lanrurugi-api/src/artist_backfill.rs` (3), `crates/lanrurugi-api/src/plugin_wizard/trial_run.rs` (1), `crates/lanrurugi-api/src/plugin_wizard/generate.rs` (1), `crates/lanrurugi-api/src/plugin_wizard/analyze_login.rs` (2), `crates/lanrurugi-api/src/subscriptions/ai_condition.rs` (1), `crates/lanrurugi-api/src/archive_split.rs` (1), `crates/lanrurugi-api/src/recommend_llm.rs` (1), `crates/lanrurugi-api/src/tankoubons.rs` (2), `crates/lanrurugi-translate/src/pricing.rs` (1). No call site's control flow changes in this task — only the entry point it uses.

- [ ] T021 [US2] Add one outcome-to-HTTP mapping in `crates/lanrurugi-api/src/common.rs` (or beside the existing error helpers) and use it at the endpoints that currently hand-map provider failures: `crates/lanrurugi-api/src/tankoubons.rs`, `crates/lanrurugi-api/src/archive_split.rs`, `crates/lanrurugi-api/src/subscriptions/ai_condition.rs`, `crates/lanrurugi-api/src/plugin_wizard/*`. Preserve each endpoint's existing status codes where a test already pins them; the change is that the *class* now decides, not a repeated `match`.

- [ ] T022 [US2] Add the quickstart S2 matrix as tests. The bound assertion must be parameterised over all nine features — the story's whole point is that the twelve previously unbounded sites are now bounded — and the failure-class assertion must enumerate every path and fail if any yields an unnamed/generic message.

**Checkpoint**: no AI action can exceed its bound under a stalled provider; every failure path maps to a named class.

---

## Phase 5: US3 — The administrator can see what the AI features consumed (P3)

**Goal**: per-feature requests, tokens and elapsed time, over day and week windows, from the product
surface.

**Independent Test**: quickstart S3's five-step matrix.

- [ ] T023 [P] [US3] Create `crates/lanrurugi-api/src/llm_usage.rs`: write one increment set per model call to `llm_usage_daily_key` (per-class failure counters, request count, elapsed, token figures, and an `unreported_token_calls` counter), using hash increments so concurrent writers cannot lose each other's updates. Set the 40-day TTL on first write of each day key.

- [ ] T024 [US3] Add `GET /api/llm/usage` in `crates/lanrurugi-api/src/llm_usage.rs` per contracts/llm-usage-api.md: `window=day|week`, all nine features always present as `reported` or `no_data`, token figures omitted rather than zeroed when unreported, `503 llm_usage_unavailable` when Redis is unreachable. Register the route in `crates/lanrurugi-api/src/lib.rs` and add the `token_guest` deny row to `crates/lanrurugi-api/policy/route_policy.csv` plus a `token_admin` decision consistent with the neighbouring AI endpoints.

- [ ] T025 [P] [US3] Add `useLlmUsage` to `apps/frontend/src/api/hooks.ts` following the file's existing TanStack Query conventions, with a type that models `no_data` and unreported tokens distinctly from zeros.

- [ ] T026 [US3] Add the usage panel to `apps/frontend/src/pages/Settings/`, per contracts/llm-usage-api.md's rendering rules (`no_data` shown as such; unreported tokens shown as "not reported", never `0`). **The computed-style obligation from this repository's UI verification rule applies**: pull `font-size`, `font-weight`, `line-height`, rendered width/height, `border-radius`, `padding` and `margin` via `getComputedStyle`/`getBoundingClientRect` from the neighbouring settings surface and diff field-by-field before claiming it matches — a screenshot is not sign-off.

- [ ] T027 [US3] Add the quickstart S3 matrix as tests in `crates/lanrurugi-api/src/llm_usage.rs`, including window arithmetic across a missing day key, the `no_data` vs zero distinction, guest denial, and `400 invalid_window`.

**Checkpoint**: quickstart S3 passes; the panel and the endpoint agree with the requests actually issued.

---

## Phase 6: US4 — An oversized page cannot swallow the generation's budget (P4)

**Goal**: bounded tool results with a continuation, plus an accumulated per-generation budget.

**Independent Test**: quickstart S4's four-step matrix.

- [ ] T028 [P] [US4] Cap the read itself in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs` — today the whole response body is materialized with no limit, so an enormous page is an unbounded allocation in the process that serves every other request. Read with a ceiling and record the source's total size as an upper bound.

- [ ] T029 [US4] Extend the tool result shape in `crates/lanrurugi-api/src/plugin_wizard/tool_loop.rs` and `fetch.rs` per data-model.md §4: `status: "truncated"`, `total_bytes`/`returned_bytes`, a quantitative truncation disclosure, and a `continuation` handle. A result under the cap must be byte-identical to today's.

- [ ] T030 [US4] Implement continuation requests in `crates/lanrurugi-api/src/plugin_wizard/fetch.rs`: a handle addresses a source and a byte range, and is re-evaluated by the T012 policy like any other request so it cannot become a way to reach a host the gate refused.

- [ ] T031 [US4] Add the accumulated per-generation budget to `crates/lanrurugi-api/src/plugin_wizard/generate.rs`'s loop: track total tool-result bytes across rounds and terminate with a specific outcome when the budget is reached, rather than silently continuing. The per-result cap and the accumulated budget are deliberately separate — five individually acceptable results are not acceptable together.

- [ ] T032 [US4] Add the quickstart S4 matrix as tests: an oversized page truncates with a disclosure, the continuation returns a further bounded portion, five large pages terminate within budget, and an ordinary page is unchanged.

**Checkpoint**: quickstart S4 passes; measured request size no longer grows proportionally to page size.

---

## Phase 7: US5 — A model output that provably fails is repaired once, not looped on (P5)

**Goal**: a validator-driven repair bound of exactly one, with the specific defect carried into the
retry.

**Independent Test**: quickstart S5's five-step matrix, driven by request counts rather than by output
shape.

- [ ] T033 [P] [US5] Create `crates/lanrurugi-api/src/repair.rs`: a helper that wraps "validate → at
  most one informed repair → re-validate", where the repair input carries the structured defect rather
  than the validator's operator-facing message. It must refuse to run at all when no validator is
  supplied (FR-023), and must never issue a second attempt (FR-021).

- [ ] T034 [US5] Wire the repair helper into `crates/lanrurugi-api/src/archive_split.rs::generate_suggestion`, where `validate_coverage` currently terminates the whole action. Feed the unassigned entries back once.

- [ ] T035 [US5] Add the missing deterministic validator to `crates/lanrurugi-api/src/tankoubons.rs` — the rename suggestions currently pass through with no check that the chapter entries correspond one-to-one with the members and that indices are in range — and give it the same one-attempt repair.

- [ ] T036 [US5] Add the quickstart S5 matrix as tests, asserting by provider-request count that a second-both-times failures case issues exactly two requests total (the original plus one repair) and never a third.

**Checkpoint**: quickstart S5 passes; no validator failure can produce more than one extra request.

---

## Phase 8: US6 — A generation can be reconstructed after the fact (P6)

**Goal**: a persisted, secret-free, retention-bounded record of what the model was shown, recoverable
after a restart.

**Independent Test**: quickstart S6's six-step matrix, including the credential scan.

- [ ] T037 [P] [US6] Create `crates/lanrurugi-api/src/plugin_wizard/record.rs` per contracts/generation-record.md: append an attempt to `generation_record_key` with placeholder-form credential fields, the bounded tool result exactly as the model saw it, the outcome class, and a timestamp; set the 30-day TTL on first append.

- [ ] T038 [US6] Write attempts from `crates/lanrurugi-api/src/plugin_wizard/generate.rs`'s loop **before** each round's request is issued, so a crash mid-round leaves the intent rather than nothing, and record the request body's credential fields in placeholder form only.

- [ ] T039 [US6] Record failed and retried rounds with their own `kind: "failed_attempt"` so no consumer can render them as content the model was shown (FR-025), in `crates/lanrurugi-api/src/plugin_wizard/record.rs`.

- [ ] T040 [US6] Add `GET /api/plugin-wizard/generate/{id}/record` in `crates/lanrurugi-api/src/plugin_wizard/mod.rs` (handler in `record.rs`), `404` when absent, using the same session-scoped authority the sibling wizard routes already require — no new permission surface. No UI: the endpoint is the whole read surface for this feature.

- [ ] T041 [US6] Add the quickstart S6 matrix as tests: ordered recovery after a restart, `404` for an unknown id, guest denial, and a credential scan over a record produced from a real generation containing a known sentinel value, asserting zero occurrences.

**Checkpoint**: quickstart S6 passes; the record answers "what was the model shown" without exposing a secret.

---

## Phase 9: Polish & Cross-Cutting Concerns

- [ ] T042 Prove SC-008: record the request bodies a provider stub receives for one generation input before and after the whole feature, and diff them. The only permitted differences are the added `target_domain` field and the bounded-result fields on truncated/refused results. Extend the T001 baseline test to cover request bodies, not just error strings.

- [ ] T043 Map `lanrurugi-translate`'s existing typed error taxonomy onto the shared vocabulary at its seam boundary in `crates/lanrurugi-translate/src/pipeline.rs` (additive only — its pipeline, budget, cache, prefetch and pruning behaviour must not change, per research.md §10). Add its usage attribution through the same ledger so the panel has all nine features.

- [ ] T044 Review `README.md`'s "Improvements over LANraragi" section against what this feature actually ships. The credential-confinement guarantee is a user-visible security property and likely warrants a line; the rest is internal and likely does not.

- [ ] T045 Run the gates: `mise run fmt-check`, `mise run check`, and (launched detached, per this repository's own rule) `mise run test`. Fix every failure; never `--no-verify`.

- [ ] T046 Self-review against the spec's FR matrix: confirm each of FR-001…FR-027 is satisfied by a named task and, where the spec demands mechanical assertion, by a named test. Any FR with no landing point is a hole in this task list, not a deferred item — report it rather than silently omitting it.

---

## Dependencies

```text
Phase 1 (T001–T003)
   │
   ├──────────────────────────────> US1 (T009–T017)   ← independent of Phase 2
   │
   └──> Phase 2 (T004–T008)
            ├──> US2 (T018–T022)
            │      ├──> US3 (T023–T027)   [needs T004's class for the failure counters]
            │      ├──> US4 (T028–T032)   [independent of US2 in principle, but T020 touches the same seam]
            │      └──> US5 (T033–T036)   [needs T004's `rejected` variant]
            └──> US6 (T037–T041)          [needs T006's key builder; T038 depends on US4's bounded result shape]
```

Story completion order for delivery: **US1 → US2 → US3 → US4 → US5 → US6**. US1 can ship alone and
closes the only security defect in the inventory, so it is the MVP.

## Parallel execution examples

**US1, first burst** (all different files, no inter-dependency):
`T009` (policy derivation) · `T010` (host matching) · `T014` (backend request field) · `T015` (frontend
request field). Then `T011 → T012 → T013` sequentially in `fetch.rs`, then `T016`, then `T017`.

**Phase 2 burst**: `T004` · `T005` · `T006` in parallel (three different files); `T007` then `T008`
sequentially afterwards.

**US3 burst**: `T023` (ledger) · `T025` (frontend hook) in parallel; `T024` depends on `T023`; `T026`
depends on `T025`; `T027` last.

**Across stories once Phase 2 is done**: `US6/T037` (new file) can proceed while `US4/T028` and
`US5/T033` are in flight — their only shared prerequisite (T004/T006) is already satisfied.

## Implementation strategy

**MVP = US1 alone.** It is one story, two modules plus one request field plus three one-line frontend
changes, and it closes the only defect in the inventory whose failure mode is silent credential
disclosure. Everything after it is quality, cost and debuggability.

**Incremental delivery after the MVP**: US2 next, because it is the change that makes the other
stories measurable (a class and a report exist before a ledger or a repair bound can depend on them).
US3 then converts US2's measurement into something an administrator can see. US4 and US5 are
independent quality improvements and can be ordered by appetite. US6 is last: it changes no behaviour,
but it is what makes every earlier story debuggable after the fact.

**Not in this plan, by design**: durable goals/plans/todos, subagent delegation, workflow fan-out,
conversation compaction, reusable instruction sets and hooks, per-agent tool scoping, and any generic
agent-loop abstraction. spec.md's Assumptions and research.md §1 record why each was tested against
the feature set's actual shape and rejected. Do not add them as "while we're here" work.
