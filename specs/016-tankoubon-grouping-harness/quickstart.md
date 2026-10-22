# Quickstart: Tankoubon Grouping Harness

Rust tests run per crate (`mise run test-crate -- lanrurugi-api`); the full `mise run test` is run
by the user.

## Q1 — Boilerplate look-alikes are not grouped (US1, SC-001/002)

Fixture pool (unit test over the pure grouping function, synthetic titles and tags):
- A: `[Creator One] Story A [中国翻訳] [DL版]`, `artist:creator one`, `language:chinese`,
  `language:translated`, `female:x`
- B: `[Creator Two] Story B [中国翻訳] [DL版]`, `artist:creator two`, same generic tags
- C1–C3: `[Creator Three] Series 1/2/3`, `artist:creator three`, `parody:p`, `character:c`
- D1–D2: cosplay sets sharing `cosplayer:k`, different `artist:`
- E1–E2: `category:anthology` issues with different contributor artists

Expected: A and B never share a group (creator gate); C1–C3 one group; D1–D2 one group; E1–E2 still
groupable; release markers contribute 0 to token overlap; a tag on every candidate weighs less than a
tag on two.

## Q2 — Review only removes, and falls back (US2)

With the `lanrurugi-llm` provider stub:
1. Valid verdicts rejecting one member → that member `keep: false`, others kept, `status: reviewed`.
2. Verdicts with an unknown id, then a valid answer → `status: repaired`, exactly 2 provider calls.
3. Two invalid answers → `status: fell_back`, exactly 2 provider calls, all members kept.
4. No key → `not_configured`, 0 calls. Settled group → `settled`, 0 calls.
5. Same members again → served from cache, 0 calls.
6. A 15-member group → 12 members sent, the 3 strongest kept untouched.
7. The user message never contains a `language:` tag or a full tag list.

## Q3 — Evidence (US3)

The suggestions response carries per-pair evidence for every returned group; the modal's "why" panel
shows it with the reviewer's reasons. (UI: the user verifies.)

## Q4 — Labels (US4)

1. Create a suggestion with one member unticked → positives for ticked pairs, negatives for
   ticked–unticked pairs.
2. "Don't suggest again" → all pairs negative.
3. Tick back a reviewer-rejected member and create → one disagreement recorded.
4. `GET …/labels` counts match; `DELETE …/labels` empties them.

## Q5 — Evaluation (US5)

1. No labels → "nothing to measure", exit 0.
2. With labels: `eval-grouping --review off` prints precision/recall for the local stage.
3. `--candidate` with `creator_gate: false` → side-by-side report; precision drops on the Q1 fixture.
4. Library data and labels unchanged after the run.

## Gates

```sh
mise run fmt-check
mise run check-crate -- lanrurugi-api lanrurugi-server lanrurugi-storage lanrurugi-llm
mise run test-crate -- lanrurugi-api
```
