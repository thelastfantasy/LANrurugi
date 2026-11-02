# Contract: Grouping Reviewer

## Input (compacted, FR-021)

System prompt: the versioned reviewing instructions (`GROUPING_REVIEW_PROMPT_VERSION`), stating
that the task is to keep only members that belong to one series, that differing creators and
release-only similarity are not a series, that ids must be echoed exactly, and the JSON shape.

User message, JSON:

```json
{
  "existing_tankoubon": { "name": "…", "member_titles": ["…"] },
  "members": [
    { "id": "m1", "title": "<release markers stripped, ≤ 80 chars>", "creators": ["…"], "tags": ["≤ 6 rarest"] }
  ]
}
```

`existing_tankoubon` only for add-to-existing suggestions (≤ 5 member titles). Never sent: full tag
lists, language/translation tags, `date_added`/`timestamp`/`source`/`rating`/`uploader`, archive
ids, file names. At most `review_max_members` (12) members; for a larger group, the members with the
lowest mean pair score are sent and the rest are kept as proposed.

## Output

`{"verdicts": [{"id": "m1", "keep": true, "reason": "≤ 1 sentence"}]}` via `json_object` mode,
temperature 0, `max_tokens = 60·N + 100`, `FeatureId::TANKOUBON_GROUPING_REVIEW`, default deadline.

## Validation and repair (FR-006/FR-007)

- The `id`s must be exactly `m1…mN`, each once. Anything else is a defect named in the repair
  prompt ("missing m3", "unknown m9", "m2 twice").
- One repair via `crate::repair::with_one_repair`; a second failure → `fell_back`.
- `keep: false` can only remove a member; ids map back only to the members sent, so an added
  archive is structurally impossible.
- For add-to-existing, only proposed new members are judged.

## Skips and fallbacks

| Condition | Status | Provider call |
| --- | --- | --- |
| All members share a creator, or all share a cosplayer | `settled` | none |
| No API key | `not_configured` | none |
| Timeout, provider error, two invalid answers | `fell_back` | the attempted ones |
| Cached review for the same members, prompt version and content | (cached status) | none |
