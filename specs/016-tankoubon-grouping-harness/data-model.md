# Phase 1 Data Model: Tankoubon Grouping Harness

New Redis keys are `LANRURUGI_`-prefixed; no existing key changes shape.

## 1. GroupingSettings (in code, ephemeral)

| Field | Default | Notes |
| --- | --- | --- |
| `title_weight` | 0.3 | as today |
| `tag_weight` | 0.7 | applied to the rarity-weighted overlap |
| `token_weight` | 0.2 | title-token Jaccard after marker stripping |
| `threshold` | 0.5 | as today |
| `creator_gate` | true | FR-001 |
| `strip_release_markers` | true | FR-002 |
| `idf_tags` | true | FR-003 |
| `review_max_members` | 12 | FR-023 |
| `review_max_tags` | 6 | FR-021 |
| `review_title_chars` | 80 | FR-021 |

`Default` is the shipped configuration; the evaluation loads a candidate as JSON with any subset of
fields overridden.

## 2. PairEvidence (ephemeral, returned to the client)

| Field | Type | Notes |
| --- | --- | --- |
| `a`, `b` | archive or tankoubon id | node ids as grouped |
| `title` | f32 | title-embedding cosine |
| `tag` | f32 | rarity-weighted tag overlap |
| `tokens` | f32 | title-token Jaccard after marker stripping |
| `score` | f32 | the blend compared with the threshold |
| `creator_match` | `same` \| `different` \| `unknown` | creators as one set per archive |
| `shared_cosplayer` | bool | the existing strong positive |
| `shared_top_tags` | list of string, ≤ 5 | the rarest shared tags, rarest first |
| `blocked_by` | optional `category` \| `creator` | the gate that blocked the pair |

Only pairs inside a returned group are returned (those that cleared every gate); blocked pairs are
counted per suggestion for the evaluation, not shipped.

## 3. GroupSuggestionId (newtype, `lanrurugi-core::ids`)

The existing ignore fingerprint (`ignored_group_suggestions::fingerprint` of sorted member ids +
existing tankoubon id), now a `#[serde(transparent)]` newtype so records, reviews and labels key on
one identity.

## 4. Review (persisted cache)

| Field | Type | Notes |
| --- | --- | --- |
| `status` | `reviewed` \| `repaired` \| `settled` \| `fell_back` \| `not_configured` | `settled` = skipped by FR-022 |
| `verdicts` | list of `{archive_id, keep, reason}` | absent unless reviewed/repaired; covers only judged members |
| `reason` | optional string | for `settled` (the shared creator/cosplayer) and `fell_back` (the failure class) |
| `prompt_version` | string | the instructions version used |
| `judged` | list of archive ids | the ≤ 12 members actually sent; others kept locally |

Key `LANRURUGI_GROUPING_REVIEW:<suggestion_id>:<prompt_version>:<content_hash>`, TTL 30 days.
`content_hash` covers the members' current titles and tags, so an edit invalidates it.
Only `reviewed`/`repaired`/`settled` are cached; failures are retried next time.

**Validation**: `verdicts` ids are exactly the judged members, each once (FR-006); the reviewer can
only mark existing members `keep: false` (FR-007).

## 5. SuggestionRecord (persisted)

Key `LANRURUGI_GROUPING_SUGGESTION:<suggestion_id>`, JSON, TTL 90 days, overwritten on each showing:
members, existing tankoubon id, pair evidence, the settings snapshot, the latest review (if any),
`shown_at`.

## 6. PairLabel (persisted, no TTL)

Key `LANRURUGI_GROUPING_LABELS` (hash). Field `<id_a>|<id_b>` with ids sorted; value JSON
`{label: "pos" | "neg", source: "created" | "dismissed", suggestion_id, at}`. Latest write wins.

Key `LANRURUGI_GROUPING_REVIEWER_DISAGREEMENTS` (hash). Field `<suggestion_id>|<archive_id>`; value
`{reviewer: "reject", user: "keep", at}`.

**Validation**: a pair is never labelled against itself; both ids are real archives (a tankoubon id
in an add-to-existing suggestion is expanded to its members at label time).

## 7. EvaluationReport (ephemeral, CLI output)

Per settings variant (`current`, `candidate`): label counts (pos/neg), for each stage (`local`,
`pipeline`) precision and recall, the number of provider calls made, and the variant's settings.
