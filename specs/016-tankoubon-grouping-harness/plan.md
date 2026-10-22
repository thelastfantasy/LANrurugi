# Implementation Plan: Local + LLM Harness for Tankoubon Grouping Suggestions

**Branch**: `016-tankoubon-grouping-harness` | **Date**: 2026-10-20 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/016-tankoubon-grouping-harness/spec.md`

## Summary

Turn the purely local tankoubon grouping into a five-stage harness, each stage inspectable and
measurable:

1. **Recall (local, unchanged in shape)** — embedding + tag + title-token scoring and the clique
   grouping in `tankoubon_grouping.rs` still propose candidate groups.
2. **Deterministic gates (local)** — a creator-conflict gate beside the existing category gate,
   release markers stripped from title tokens, and rarity-weighted (IDF) tag overlap. The pair test
   now returns its evidence instead of a bare boolean.
3. **AI review (lazy, compacted)** — a new endpoint reviews one suggestion through `lanrurugi_llm`'s
   `CallSpec` seam (new `FeatureId::TANKOUBON_GROUPING_REVIEW`), with a compact input (short member
   ids, cleaned titles, creators, the rarest few tags), a deterministic validator (exactly the input
   members, each once), one repair via `crate::repair::with_one_repair`, and a local fallback. Groups
   the local evidence settles skip review. The frontend reviews only the group in view plus the next
   two; reviews are cached by member set + prompt version.
4. **Records and labels** — each shown suggestion is recorded with evidence, settings and review; the
   user's create/untick/re-tick/dismiss actions are stored as pair labels.
5. **Evaluation** — a `lanrurugi-server eval-grouping` subcommand replays the pipeline over labelled
   archives with current vs candidate settings and reports precision/recall per stage.

## Technical Context

**Language/Version**: Rust (workspace, edition 2021); TypeScript + React 19 frontend.

**Primary Dependencies**: Axum, Tokio, the existing async Redis pool, `lanrurugi-llm` (014's seam),
`lanrurugi-recommend` (embeddings), clap (CLI). New third-party dependencies: none.

**Storage**: Redis, new `LANRURUGI_`-prefixed keys only: suggestion records (TTL 90 days), review
cache (TTL 30 days), pair labels (no TTL; user-deletable).

**Testing**: `mise run test-crate -- lanrurugi-api` (unit tests for gates, IDF, compaction,
validator, labels; Redis-gated round trips), `lanrurugi-llm`'s provider stub pattern for the
reviewer, a fixture-library test for SC-001/SC-002. The full suite is run by the user.

**Target Platform**: the existing single-process server and SPA.

**Project Type**: web service + SPA in the existing monorepo.

**Performance Goals**: the first suggestion appears no later than today (review is never on that
path); IDF adds one pass over candidate tags; evaluation of 10,000 archives' labels under one minute.

**Constraints**: one review's input under ~1,500 tokens for ten members (FR-021, SC-008); at most
N + 2 reviews for N groups viewed (SC-004); the reviewer can only remove members (FR-007).

**Scale/Scope**: one module reworked (`tankoubon_grouping.rs`), one new module per concern (review,
labels/records, evaluation), one modal reworked, one CLI subcommand.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Verdict |
| --- | --- | --- |
| I. Legacy data compatibility | No legacy key read differently or rewritten | **Pass** — new keys are `LANRURUGI_`-prefixed; the existing ignored-suggestions hash is read as before |
| II. API contract fidelity | Legacy endpoints unchanged | **Pass** — `/api/tankoubons/ai-group-suggestions*` is LANrurugi-only; its response gains additive fields; new endpoints are additive |
| III. Resource-conscious single process | CPU work off the reactor, no unbounded parallelism | **Pass** — scoring stays inside the existing `spawn_blocking`; IDF is one linear pass; no new rayon pools |
| IV. Plugin sandbox | — | Not touched |
| V. Secrets & trust boundaries | Provider keys server-side; what is sent is deliberate | **Pass** — the review sends only cleaned titles, creators and a few tags, through the existing seam; no key reaches the browser |
| VI. Phased scope | Phase 2 not smuggled in | **Pass** — Phase 1 feature on 014's seam |
| VII. Frontend discipline | Theme-aware colours, user-verified UI | **Pass with obligation** — new card states reuse existing legacy classes and theme files; visual sign-off is the user's |
| Tech constraints: newtype ids | Domain ids are newtypes | **Pass** — `GroupSuggestionId` newtype for the suggestion fingerprint |
| Cost-aware defaults | Metered calls conservative by default | **Pass** — review is lazy (in view + 2), skips settled groups, caches by member set |

**Gate result: pass.**

**Post-design re-check**: still pass. The one point made explicit: labels have no TTL (FR-017) —
they are user data the evaluation depends on, so the plan adds a delete-all endpoint rather than a
timer, keeping them within Principle I's "user-trust" spirit.

## Project Structure

### Documentation (this feature)

```text
specs/016-tankoubon-grouping-harness/
├── plan.md
├── research.md
├── data-model.md
├── quickstart.md
├── contracts/
│   ├── grouping-api.md       # suggestions (extended), review, labels
│   ├── review-contract.md    # reviewer input/output, validator, fallback
│   └── eval-cli.md           # eval-grouping subcommand
└── tasks.md
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-core/src/ids.rs                     # + GroupSuggestionId
├── lanrurugi-llm/src/usage.rs                    # + FeatureId::TANKOUBON_GROUPING_REVIEW
├── lanrurugi-storage/src/
│   ├── keys.rs                                   # + record / review-cache / labels keys
│   └── grouping_labels.rs                        # NEW — pair labels + suggestion records repo
├── lanrurugi-api/src/
│   ├── tankoubon_grouping.rs                     # gates, IDF, evidence, settings struct, records
│   ├── tankoubon_grouping/                       # NEW submodules
│   │   ├── release_markers.rs                    #   the one marker list + stripping
│   │   ├── review.rs                             #   compaction, prompt, validator, endpoint
│   │   └── labels.rs                             #   label endpoints
│   ├── llm_prompts.rs                            # + grouping review prompt (versioned)
│   └── policy/route_policy.csv                   # + new routes
├── lanrurugi-server/src/
│   ├── main.rs                                   # + eval-grouping subcommand
│   └── cli/eval_grouping.rs                      # NEW
apps/frontend/src/
├── api/{hooks.ts,types.ts}                       # review + labels hooks, evidence types
└── pages/Library/AiSmartTankoubonModal.tsx       # review state, evidence panel, labels
```

**Structure Decision**: `tankoubon_grouping.rs` (1,675 lines) becomes a directory module so the new
concerns do not grow one file further; the existing scoring code stays where it is.

## Complexity Tracking

> No constitution violations.
