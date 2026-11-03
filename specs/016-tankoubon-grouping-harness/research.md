# Phase 0 Research: Tankoubon Grouping Harness

Each decision: **Decision / Rationale / Alternatives considered**.

## 1. Creator-conflict gate

**Decision**: Treat `artist:` and `circle:` values together as one creator set per archive. If both
sets are non-empty and disjoint, the pair is blocked — unless either archive is
`category:anthology` or the pair shares a `cosplayer:` value (which already short-circuits to
similar). Implemented beside `category_conflict`, checked before scoring.

**Rationale**: "different known creators" is the strongest available evidence of "different
series", and today nothing counts against a pair. Mirrors the category gate's proven shape: a hard
gate, not a weight that other shared tags could out-vote. Combining artist and circle avoids
blocking a circle release against its member artist's solo work.

**Alternatives considered**: a negative weight for differing creators (rejected: shared boilerplate
could still out-vote it — the exact failure being fixed); artist only (rejected: misses
circle-credited works).

## 2. Release markers

**Decision**: One list (`release_markers.rs`) of normalized marker tokens — translation (中国翻訳,
中国語, 英訳, 漢化, translated…), edition (DL版, digital, 電子版…), censorship (無修正, uncensored,
decensored…), and format/page tokens — removed from title tokens before overlap is measured. Matching
is case-insensitive on the whole token after bracket splitting.

**Rationale**: these tokens appear across unrelated works and were a direct cause of the reported
false group. One list keeps them auditable and extendable from evaluation findings.

**Alternatives considered**: dropping all bracketed tokens (rejected: creator handles are bracketed
and are the signal the token bonus exists for); learning markers from frequency (folded in via
rarity — §3 — for tags; titles keep an explicit list because a single title token's frequency is too
noisy at small library sizes).

## 3. Rarity-weighted tag overlap

**Decision**: Weighted Jaccard over the existing Jaccard-eligible tag set, each tag weighted by
`idf = ln((N + 1) / (df + 1)) + 1` where N is the candidate pool size and df the number of candidates
carrying it. Score = Σ idf(shared) / Σ idf(union). Computed once per request over the candidate pool
inside the existing blocking task.

**Rationale**: makes library-wide tags (language, translated, common genre tags) near-worthless while
a shared rare tag (a parody, a character, a creator) dominates — matching how a human reads it. The
`+1` smoothing keeps a tag every archive carries from going to exactly zero and keeps scores in the
same 0–1 range the threshold already uses.

**Alternatives considered**: a static stop-list of generic tags (rejected: library-specific and
needs maintenance; IDF adapts); BM25-style saturation (rejected: tag sets are binary, no term
frequency).

**Threshold**: kept at 0.5 initially. Weighted scores for genuine series stay high because their
shared tags are the rare ones; this is checked by the fixture test (SC-002) and is exactly what the
evaluation (§8) exists to retune.

## 4. Evidence instead of a boolean

**Decision**: The pair test returns `PairEvidence { title, tag, tokens, creator_match, blocked_by,
shared_top_tags, score, similar }`. Grouping uses `similar`; suggestions carry the evidence of every
member pair (groups are small; cliques average 2–6 members).

**Rationale**: FR-012 and the evaluation both need the numbers; computing them once avoids a second
code path that could drift from the real decision.

## 5. Review endpoint, laziness and caching

**Decision**: `POST /api/tankoubons/ai-group-suggestions/review` with the suggestion's members (and
existing tankoubon id). The frontend calls it for the group in view and prefetches the next two.
Result cached in Redis under `GroupSuggestionId` (fingerprint of members + tankoubon) + prompt
version + a hash of the members' current titles/tags, TTL 30 days.

**Rationale**: keeps the suggestions call fast and free (SC-003), bounds cost by what the user
actually looks at (SC-004), and makes re-opening the modal free. Keying on content means an edited
title invalidates naturally.

**Alternatives considered**: reviewing all groups server-side when suggestions are computed
(rejected: 43 calls before the first card); a background job (rejected: reviews groups nobody looks
at).

## 6. Compacted reviewer input

**Decision**: Members are sent as `m1…mN` with `title` (release markers stripped, ≤ 80 chars),
`creators` (≤ 3) and `tags` (the ≤ 6 rarest Jaccard-eligible tags, never language/translation/
noise namespaces). For an add-to-existing suggestion, the tankoubon is sent as its name plus ≤ 5
member titles, and only proposed new members are judged. At most 12 members per review; a larger
group sends the 12 with the weakest mean pair score and keeps the rest. Output:
`{"verdicts":[{"id":"m1","keep":true,"reason":"…"}]}`, `json_object` mode, temperature 0,
`max_tokens = 60·N + 100`.

**Rationale**: ~100 tokens per member keeps a ten-member review near 1,000–1,500 input tokens
(SC-008); short ids save tokens and make the validator trivial; the weakest members are the ones
worth a second opinion.

**Alternatives considered**: sending full tag lists (rejected by the user: token cost); a
pairwise-question format (rejected: O(n²) calls/tokens).

## 7. Settled groups skip review

**Decision**: Skip review when all members share at least one creator value in common, or all share
one cosplayer value. Status `settled` with the shared value as the reason.

**Rationale**: the local evidence already decides these; the reviewer would only cost tokens.

## 8. Labels, records and the evaluation

**Decision**:
- Labels: Redis hash keyed by the sorted archive pair, value `{label: pos|neg, source, at}`; latest
  wins. Sources: `created` (ticked–ticked +, ticked–unticked −), `dismissed` (all pairs −, recorded
  by the ignore handler itself), plus `reviewer_disagreement` entries when the user ticked back a
  member the reviewer rejected.
- Records: one JSON per `GroupSuggestionId` with evidence, settings snapshot and review, TTL 90 days.
- Evaluation: `lanrurugi-server eval-grouping [--candidate settings.json] [--review cached|live|off]`.
  It loads labels, restricts the candidate pool to labelled archives, runs local grouping (and the
  review stage per mode) for current and candidate settings, and reports pair precision/recall:
  precision = labelled-positive among proposed labelled pairs, recall = proposed among labelled
  positives. `cached` reuses the review cache only; `live` may call the provider and reports how many
  calls it made; `off` measures the local stage alone.

**Rationale**: pair labels are what the user's actions actually express, and pair metrics are
directly comparable across groupings with different shapes. Restricting to labelled archives keeps
the run fast and its numbers meaningful.

**Alternatives considered**: group-level labels (rejected: a partly-correct group carries both
right and wrong pairs); automatic weight search (out of scope — a human decides).

## 9. Settings as data

**Decision**: A `GroupingSettings` struct (weights, threshold, token weight, creator gate on/off,
marker stripping on/off, IDF on/off, review caps) whose `Default` is the shipped configuration. The
request path uses `Default`; the evaluation loads a candidate from JSON.

**Rationale**: the evaluation must run the exact production code with different numbers; constants
scattered through the module cannot be varied without editing code.

## 10. Feature id

**Decision**: `FeatureId::TANKOUBON_GROUPING_REVIEW` (`tankoubon_grouping_review`) appended to
`FeatureId::ALL`, so the AI usage panel and ledger cover it.

## Resolved unknowns

| Unknown | Resolution |
| --- | --- |
| Whether IDF breaks the threshold | Kept at 0.5; fixture test + evaluation are the check (§3) |
| How to bound review cost | Lazy in-view + 2, settled skip, cache, compact input (§5–§7) |
| Where labels come from without new UI | Existing create/untick/dismiss actions (§8) |
