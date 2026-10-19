# Implementation Plan: Agent-Harness Hardening for LANrurugi's AI Call Sites

**Branch**: `014-ai-harness-hardening` | **Date**: 2026-10-16 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/014-ai-harness-hardening/spec.md`

## Summary

LANrurugi's AI surface is 14 server-side model calls across 9 features, one 3-provider
translation pipeline, four local model classes, and one browser-direct path. Measured against the
agent-harness construct list, exactly four constructs are both applicable and missing, one is
partially present, and six are structurally inapplicable and stay out of scope.

This plan delivers the four missing constructs and completes the partial one, **additively**:

1. **A tool gate** for the one place where the model chooses a network destination and the server
   attaches the user's real credential to it (currently unconstrained — a
   prompt-injection-to-exfiltration path).
2. **A real model-call seam**: one failure vocabulary, a deadline that covers retries, retry-once
   for transient classes only, and per-call token/elapsed measurement — replacing fourteen
   independently-written error maps and the absence of any bound at twelve of them.
3. **Per-feature usage attribution** readable from the product surface (today: 1 of 9 features).
4. **Bounded tool results** (prune + continuation), the construct that decides whether a
   multi-round generation completes at a usable cost.
5. **Bounded repair**: one attempt, driven only by a deterministic validator's verdict — the two
   validators that exist gain the loop, and the wizard's repair cycle gains a server-side record.

Every change is either a new module, a new Redis key namespace, or an additive field/enum variant.
No existing success path changes behavior; SC-008 makes that a tracked requirement rather than an
intention.

## Technical Context

**Language/Version**: Rust (workspace, edition 2021) for the backend; TypeScript + React 19 for the
one frontend app.

**Primary Dependencies**: Tokio, Axum, the existing async Redis client, `reqwest`. New third-party
dependencies: none required. Address-class classification uses `std::net`; host resolution uses the
resolver path already reachable through `reqwest`/`tokio`.

**Storage**: Redis, reused as-is (constitution: a second store would require an amendment). New key
namespaces only; no existing key's shape changes.

**Testing**: `cargo test` through `mise run test-crate` / `mise run test`; real network round-trips
against `127.0.0.1` fixtures for fetch-path tests (the existing pattern in
`crates/lanrurugi-api/src/plugin_wizard/fetch.rs`); Redis-gated integration tests that skip when
`LANRURUGI_TEST_REDIS_URL` is unset (existing convention).

**Target Platform**: Linux server, single process, single-tenant deployment.

**Project Type**: Web service (Rust backend) + SPA frontend in one monorepo.

**Performance Goals**: no measurable change to any existing AI action's ordinary-path latency or
cost. Added per-call work is one host resolution and a handful of counter increments.

**Constraints**: the new per-call bound must not truncate a legitimate long-form generation. A real
generation has been observed taking 80+ seconds for its final round, and the wizard's own overall
bound is 300s, so the per-call ceiling is set above the longest legitimate single round and is never
used as a shorter overall deadline. Tool-result bounding must not degrade a page that already fits.

**Scale/Scope**: 14 call sites to register in one seam; 9 feature ids in the usage ledger; 1
persisted record type with a bounded retention window.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Verdict |
| --- | --- | --- |
| I. Legacy data & user-trust compatibility | No change to archive identity, tag, progress or grouping data; new Redis keys additive and non-colliding | **Pass** — adds `llm_usage:*` and `generation_record:*` namespaces only; no legacy key read or rewritten; retention is per-key TTL, never a sweep over shared data |
| II. API contract fidelity | Legacy-shaped endpoints unchanged; new capabilities additive | **Pass** — every touched endpoint (`/api/plugin-wizard/*`, `/api/tankoubons/*/ai/*`, `/api/subscriptions/condition/ai`, `/api/archives/*/split/*`) is a LANrurugi-only addition with no legacy counterpart. `/api/llm/usage` is a new path. Error *bodies* on those endpoints gain a machine-readable class; success shapes are untouched |
| III. Resource-conscious, genuinely concurrent single process | No loop-of-single-item `spawn_blocking`; CPU work batched via rayon; no new long-lived process | **Pass** — no CPU-bound batch work added; host resolution is async I/O. Bounding tool results strictly *reduces* peak memory, which is currently unbounded on a large fetched page |
| IV. Sandboxed plugin extensibility | Plugin execution stays inside the Deno permission model | **Pass, strengthened** — the wizard's page fetch is today the one network capability that bypasses the plugin permission model. FR-001…FR-006 bring it under an explicit policy for the first time |
| V. Secrets & network trust boundaries | Provider keys server-side only; secrets never browser-readable | **Pass, strengthened** — FR-002/FR-006 confine a substituted credential to its own site; FR-026 keeps it out of the persisted record. No credential becomes browser-readable, and the record is session-scoped exactly like the wizard itself |
| VI. Phased scope discipline | Phase 2 concerns must not be smuggled into Phase 1 work nor gate it | **Pass** — cross-phase infrastructure. The only Phase 2 surface touched is `lanrurugi-translate`, additively (failure-vocabulary boundary + attribution). Every story is independently deliverable |
| VII. Frontend discipline & legacy UI fidelity | New UI verified against real computed style, not eyeballed | **Pass with obligation** — the only new UI is the usage panel; its task carries the mandatory computed-style comparison against the settings surface it sits in |

**Gate result: pass, no violations, no complexity-tracking entries required.**

**Post-design re-check (after `research.md`, `data-model.md` and `contracts/`)**: still pass, with one
point made explicit rather than assumed. The design stores two new Redis namespaces; Principle I
forbids touching legacy data, and the chosen names (`LANRURUGI_LLM_USAGE:*`,
`LANRURUGI_GENERATION_RECORD:*`) are `LANRURUGI_`-prefixed per the existing convention for keys with
no legacy counterpart, so no upstream key can ever collide with them. Principle V also gains a
concrete guarantee it did not have before this feature: the outbound-fetch contract now makes
"a substituted credential is never sent off-site" a testable invariant rather than a property of the
code's good intentions.

## Project Structure

### Documentation (this feature)

```text
specs/014-ai-harness-hardening/
├── plan.md              # This file
├── research.md          # Phase 0 output — construct-adoption decisions and alternatives
├── data-model.md        # Phase 1 output — entities, keys, states, retention
├── quickstart.md        # Phase 1 output — runnable validation scenarios
├── contracts/           # Phase 1 output — four interface contracts
│   ├── outbound-fetch-policy.md
│   ├── llm-call-contract.md
│   ├── llm-usage-api.md
│   └── generation-record.md
└── tasks.md             # Phase 2 output (/speckit-tasks command)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-llm/src/
│   ├── lib.rs              # existing seam entry points; re-exports the new vocabulary
│   ├── error.rs            # NEW — one failure vocabulary; Display preserves today's messages
│   ├── retry.rs            # NEW — deadline-aware, class-aware retry-once policy
│   └── usage.rs            # NEW — provider usage extraction + per-call report
├── lanrurugi-api/
│   ├── policy/route_policy.csv          # + /api/llm/usage guest deny
│   └── src/
│       ├── plugin_wizard/fetch.rs       # + policy evaluation (3 points), byte-capped body read
│       ├── plugin_wizard/tool_loop.rs   # + denial surfacing, result bounding, continuation
│       ├── plugin_wizard/generate.rs    # + target_domain, accumulated budget, record writes
│       ├── plugin_wizard/record.rs      # NEW — generation record persistence + retention
│       ├── llm_call.rs                  # NEW — the 9 features' single entry point to the seam
│       ├── llm_usage.rs                 # NEW — ledger + GET /api/llm/usage
│       ├── repair.rs                    # NEW — validate, then at most one informed repair
│       ├── archive_split.rs             # + one bounded repair on coverage failure
│       ├── tankoubons.rs                # + rename validators, one bounded repair
│       ├── scripts.rs                   # + feature attribution only
│       ├── artist_backfill.rs           # + feature attribution only
│       ├── recommend_llm.rs             # + feature attribution only
│       └── subscriptions/ai_condition.rs # + feature attribution only
├── lanrurugi-translate/src/pipeline.rs  # + boundary mapping of the new failure vocabulary
└── lanrurugi-storage/src/keys.rs        # + new key namespaces

apps/frontend/src/
├── api/hooks.ts            # + useLlmUsage
└── pages/Settings/         # + usage panel (computed-style verified)
```

**Structure Decision**: existing Cargo workspace and the single frontend app, per the
constitution's repository-layout constraint. `lanrurugi-llm` grows three modules because the seam
belongs where all fourteen callers already depend. The api crate grows four because attribution,
the ledger endpoint, the repair helper and the record are API-layer concerns that `lanrurugi-llm`
must not learn about — it deliberately knows nothing about features or HTTP.

**Deliberate non-structure**: no `crates/lanrurugi-harness` abstraction over the nine AI features.
Two of the nine have a loop; the rest are single calls. A second framework for two users is exactly
the over-engineering this spec's Assumptions section rejects.

## Phased delivery

The six stories are independent by construction (each has its own Independent Test in the spec).
Ordering below is by risk, so that the security slice can land and be verified alone.

- **Phase A (US1)** — outbound fetch policy. Self-contained in two modules plus one request field.
- **Phase B (US2)** — LLM seam. Touches `lanrurugi-llm` plus one mechanical attribution line per call
  site; no call site's control flow changes.
- **Phase C (US3)** — ledger, endpoint, panel (depends on B's measurement).
- **Phase D (US4, US5, US6)** — result bounding, bounded repair, generation record. Mutually
  independent; result bounding is a prerequisite for the record's usefulness, not for its storage.

## Parallel-work sequencing note

A separate body of work is in flight on this branch and shares the working tree. Implementation
order therefore also minimises file overlap: Phase A and Phase B touch files no other feature owns
(`plugin_wizard/fetch.rs`, `plugin_wizard/tool_loop.rs`, `lanrurugi-llm/*`), while Phase C's frontend
panel and Phase D's `plugin_wizard/generate.rs` edits are the highest-overlap items and land last.
This is a scheduling fact, not a design constraint — no story is blocked on the other body of work.

## Complexity Tracking

> No constitution violations. This section is intentionally empty.
