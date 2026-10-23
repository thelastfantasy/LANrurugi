---

description: "Task list for feature 016-tankoubon-grouping-harness"
---

# Tasks: Local + LLM Harness for Tankoubon Grouping Suggestions

**Input**: Design documents from `/specs/016-tankoubon-grouping-harness/`

**Prerequisites**: plan.md, spec.md, research.md, data-model.md, contracts/, quickstart.md

**Tests**: Included where a success criterion demands a mechanical assertion — SC-001/SC-002 (the
fixture pool), SC-005 (reviewer can only remove), the validator and fallbacks, labels, and the
evaluation metrics. Rust tests run per crate (`mise run test-crate`); the full `mise run test` is the
user's.

## Format: `[ID] [P?] [Story] Description`

## Phase 1: Setup

- [X] T001 Declare the submodules of `crates/lanrurugi-api/src/tankoubon_grouping.rs` (`mod release_markers; mod review; mod labels;`) with files under `crates/lanrurugi-api/src/tankoubon_grouping/`; the existing module stays where it is.
- [X] T002 [P] Add the `GroupSuggestionId` newtype in `crates/lanrurugi-core/src/ids.rs` and make `lanrurugi_storage::ignored_group_suggestions::fingerprint` return it (call sites unchanged in behaviour).
- [X] T003 [P] Add `FeatureId::TANKOUBON_GROUPING_REVIEW` (`tankoubon_grouping_review`) to `crates/lanrurugi-llm/src/usage.rs` and `FeatureId::ALL`; add its name to the usage panel strings (`settings.llmUsageFeature_tankoubon_grouping_review`) in `apps/frontend/src/i18n/locales/{zh,en,ja}.json`.
- [X] T004 [P] Add key builders in `crates/lanrurugi-storage/src/keys.rs`: `grouping_review_key(id, prompt_version, content_hash)` (TTL 30 days), `grouping_suggestion_key(id)` (TTL 90 days), `GROUPING_LABELS_KEY`, `GROUPING_DISAGREEMENTS_KEY`, with documented TTL constants.

## Phase 2: Foundational

- [X] T005 Introduce `GroupingSettings` (data-model.md §1) with a `Default` equal to today's constants, and thread it through `is_similar_pair` / `group_by_similarity` in `crates/lanrurugi-api/src/tankoubon_grouping.rs` with no behaviour change; the request path uses `GroupingSettings::default()`.
- [X] T006 Replace the boolean pair test with `pair_evidence(...) -> PairEvidence` (data-model.md §2); grouping uses `evidence.similar`. Extend each returned suggestion with `suggestion_id` and `evidence` for its member pairs (contracts/grouping-api.md), additive only.

## Phase 3: US1 — Release markers and generic tags no longer make a series (P1)

**Goal**: deterministic gates. **Independent test**: quickstart Q1.

- [X] T007 [P] [US1] Create `crates/lanrurugi-api/src/tankoubon_grouping/release_markers.rs`: the one marker list (translation, edition, censorship, format/page markers; research.md §2) and `strip(tokens)`; apply it in `title_tokens` when `strip_release_markers` is on. Unit tests: markers removed, creator handles kept.
- [X] T008 [US1] Add `creator_conflict` (artist + circle as one set; both non-empty and disjoint; not when either is `category:anthology`; a shared cosplayer still wins) beside `category_conflict`, gated by `creator_gate`, recorded as `blocked_by: creator`. Unit tests for every edge case in spec.md.
- [X] T009 [US1] Rarity-weighted tag overlap: build a `TagRarity` (idf per tag over the candidate pool, research.md §3) once per request inside the blocking task; use weighted Jaccard when `idf_tags` is on; `shared_top_tags` = rarest shared tags. Unit tests: a tag on every candidate weighs less than a tag on two.
- [X] T010 [US1] Fixture test (quickstart Q1) over `group_by_similarity` with synthetic vectors/titles/tags: A/B never grouped, C1–C3 grouped, D1–D2 grouped, E1–E2 groupable.

## Phase 4: US2 — An AI reviewer checks the group in front of the user (P2)

**Goal**: lazy, compacted, validated review. **Independent test**: quickstart Q2.

- [X] T011 [P] [US2] In `crates/lanrurugi-api/src/tankoubon_grouping/review.rs`: `settled_reason(members)` (all share a creator or all share a cosplayer → reason) and `compact_input(...)` (m1…mN, stripped/truncated titles, ≤ 3 creators, ≤ 6 rarest eligible tags, never language/translation/noise tags; ≤ 12 members chosen by weakest mean pair score; add-to-existing context). Unit tests incl. "no `language:` tag in the payload" and the 12-member cap.
- [X] T012 [P] [US2] Add the versioned reviewing prompt (`GROUPING_REVIEW_PROMPT_VERSION`, `grouping_review_system()`) to `crates/lanrurugi-api/src/llm_prompts.rs` per contracts/review-contract.md.
- [X] T013 [US2] In `review.rs`: `validate_verdicts` (exactly m1…mN once each, defect text), `run_review(state, suggestion)` via `json_chat` with `CallSpec::new(FeatureId::TANKOUBON_GROUPING_REVIEW)`, `crate::repair::with_one_repair`, statuses `reviewed`/`repaired`/`settled`/`fell_back`/`not_configured`, content hash, cache read/write (only success/settled cached), dropping members that no longer exist.
- [X] T014 [US2] Add `POST /tankoubons/ai-group-suggestions/review` (contracts/grouping-api.md) to the grouping router; deny rows for `token_guest` in `crates/lanrurugi-api/policy/route_policy.csv`; never an HTTP error for a review failure.
- [X] T015 [US2] In the suggestions handler, attach `review` inline when it is settled or cached — no provider call on this path.
- [X] T016 [US2] Tests: validator cases; compaction; provider-stub round trip in `review.rs` for reviewed / repaired (2 calls) / fell_back (2 calls) / not_configured (0) / cached (0); assert verdict ids ⊆ input (SC-005).
- [X] T017 [P] [US2] Frontend types and hooks: `GroupSuggestionReview`, `PairEvidence` in `apps/frontend/src/api/types.ts`; `useGroupSuggestionReview(suggestion, enabled)` (staleTime ∞) in `apps/frontend/src/api/hooks.ts`.
- [X] T018 [US2] `apps/frontend/src/pages/Library/AiSmartTankoubonModal.tsx`: review the card in view and prefetch the next two; status line (reviewed / settled with reason / not reviewed and why); rejected members start unticked with their reason unless the user already toggled them; "no series here" state when the reviewer keeps fewer than the minimum. Visual sign-off is the user's.

## Phase 5: US3 — Why a group was suggested (P3)

**Goal**: evidence in the card. **Independent test**: quickstart Q3.

- [X] T019 [US3] Evidence panel in `AiSmartTankoubonModal.tsx`: a "why" toggle listing per-pair title/tag/token scores, creator match and shared rare tags, plus per-member reviewer reasons. Theme-aware via existing classes; user verifies.

## Phase 6: US4 — The user's decisions become labels (P4)

**Goal**: pair labels and suggestion records. **Independent test**: quickstart Q4.

- [X] T020 [P] [US4] `crates/lanrurugi-storage/src/grouping_labels.rs`: `GroupingLabelsRepository` (record pair labels, disagreements, counts, delete-all, list-all for the evaluation) and `save_suggestion_record`.
- [X] T021 [US4] `crates/lanrurugi-api/src/tankoubon_grouping/labels.rs`: `POST/GET/DELETE /tankoubons/ai-group-suggestions/labels`; the ignore handler records negative pair labels; the suggestions handler saves a SuggestionRecord per returned suggestion. Policy rows.
- [X] T022 [US4] Frontend: after a successful create in `AiSmartTankoubonModal.tsx`, post `kept`/`unticked`/`reviewer_rejected_kept` (best effort, never blocks the create).
- [X] T023 [US4] Redis-gated tests: created → pos/neg pairs; dismissed → negatives; disagreement; counts; delete-all.

## Phase 7: US5 — Changes are measured before they ship (P5)

**Goal**: offline evaluation. **Independent test**: quickstart Q5.

- [X] T024 [US5] `pub` evaluation core in `crates/lanrurugi-api/src/tankoubon_grouping/eval.rs`: load labels, build the labelled candidate pool, run `group_by_similarity` per settings variant, apply cached/live/off review, compute pair precision/recall per stage, count provider calls.
- [X] T025 [US5] `eval-grouping` subcommand in `crates/lanrurugi-server/src/main.rs` + `crates/lanrurugi-server/src/cli/eval_grouping.rs` (contracts/eval-cli.md): `--candidate` JSON, `--review off|cached|live`, side-by-side table.
- [X] T026 [US5] Unit tests for the metrics (precision/recall on hand-built groupings and labels; "nothing to measure" with no labels).

## Phase 8: Polish

- [X] T027 Strings for every new modal state and the evidence panel in `apps/frontend/src/i18n/locales/{zh,en,ja}.json`.
- [X] T028 Docs: CLAUDE.md feature list; note the 12th feature id in `specs/014-ai-harness-hardening/data-model.md` §0; README improvements line if user-visible behaviour warrants it.
- [X] T029 Gates: `mise run fmt-check`; `mise run check-crate -- lanrurugi-api lanrurugi-server lanrurugi-storage lanrurugi-llm lanrurugi-core`; `mise run test-crate` for `lanrurugi-api` and `lanrurugi-storage`; frontend `tsc -b` and eslint on changed files. The full `mise run test` is handed to the user.
- [X] T030 Implementation record in this file: what was verified, what was not, deviations.

## Dependencies

```text
Phase 1 → Phase 2 → US1 (T007–T010)
                  → US2 (T011–T018)  [T013 needs T003, T004; T018 needs T017]
                  → US3 (T019)       [needs T006 evidence and T018's card]
                  → US4 (T020–T023)  [T021 needs T004, T020]
                  → US5 (T024–T026)  [needs T005 settings, T020 labels]
```

Delivery order: US1 → US2 → US3 → US4 → US5. US1 alone fixes the reported case with no provider.

## Parallel execution examples

- Setup: T002 · T003 · T004 in parallel.
- US1: T007 in parallel with T008/T009 (different files); T010 last.
- US2: T011 · T012 · T017 in parallel; then T013 → T014 → T015 → T016; T018 after T017.
- US4: T020 in parallel with US2 work; T021 → T022 → T023.

## Implementation strategy

**MVP = US1**: deterministic gates fix the reported false group for every user, no AI cost.
Then US2 (precision with bounded cost), US3 (evidence), US4 (labels), US5 (evaluation). US4 should
land before tuning anything, since without labels no change can be measured.

## Implementation Record (2026-10-20)

**Verified** (single-crate gates, detached, through `mise run`):
- `check-crate` for `lanrurugi-api`, `lanrurugi-server`, `lanrurugi-storage`, `lanrurugi-llm`,
  `lanrurugi-core`, `lanrurugi-translate`: clean, no warnings. `mise run fmt` applied.
- `test-crate lanrurugi-api`: every unit test passes, including the pre-existing grouping regression
  tests (anthology, shared cosplayer, title-token borderline, chain merge, existing-tankoubon nodes)
  under the new creator gate and IDF weighting — none needed recalibrating. New: gate edge cases,
  rarity, release-marker overlap, the Q1 fixture, compaction/validator/metrics/labels units.
- `tests/grouping_review.rs` (provider stub): reviewed (1 call, only removes), repaired (2 calls),
  an archive outside the group → `fell_back` after exactly 2 calls (SC-005), settled (0 calls),
  cached (0 calls on the second ask; Redis-gated).
- `test-crate lanrurugi-storage`: labels round trip through Redis (created/dismissed/disagreement/
  counts/delete-all).
- Frontend: eslint on the changed files clean. `tsc -b` was not run (memory-heavy; left to the user's
  full gate) — the editor reports no diagnostics on the changed files.

**Not verified:** the full `mise run test` (handed to the user); the modal's look and behaviour in a
browser (status line, unticked rejected members with reasons, the "why" panel) — the user's sign-off;
a live provider review and a real `eval-grouping` run over real labels (no labels exist yet).

**Deviations from plan/contracts:**
- `PairEvidence`/`GroupReview` types live in `apps/frontend/src/api/hooks.ts` beside the existing
  `AiGroupSuggestion`, not in `types.ts`.
- The review endpoint returns `{suggestion_id, review}` (contracts/grouping-api.md updated); the
  labels body carries no `suggestion_id` — it is derived server-side.
- Labels and suggestion records use the config pool through `labels_repo(state)`, not a new
  `AppState` field.
- `eval-grouping` reports groups reviewed by the provider, not raw calls, and does not add existing
  tankoubons as nodes (contracts/eval-cli.md updated).
- The prompt version is `v2`: the harness audit below appended the shared "data, not instructions"
  rule to it, which changes what the model sees.
- The reviewer now goes through the shared `repair::json_chat_checked` (see 014's record), so a
  reply in the wrong JSON shape is also repaired once rather than falling back immediately.

