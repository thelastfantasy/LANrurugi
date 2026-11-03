# Contract: Grouping Suggestions, Review and Labels

All routes are under `/api`, LANrurugi-only (no legacy counterpart), session or admin token, denied
to read-only tokens and guest visitors like the existing grouping routes.

## `POST /tankoubons/ai-group-suggestions` (extended, additive)

Each suggestion gains:

```json
{
  "archive_ids": ["a1", "a2"],
  "existing_tankoubon_id": null,
  "suggestion_id": "<GroupSuggestionId>",
  "evidence": [
    { "a": "a1", "b": "a2", "title": 0.71, "tag": 0.62, "tokens": 0.4, "score": 0.73,
      "creator_match": "same", "shared_cosplayer": false, "shared_top_tags": ["parody:x"] }
  ],
  "review": { "status": "settled", "reason": "artist:x" }
}
```

`review` is present only for a settled group (all members share a creator or a cosplayer). For
every other card the client asks the review endpoint when the card comes into view; the endpoint
answers from the review cache when it can and counts that in the usage ledger as a local cache hit
(`local_cache_hits`), so the count reflects cards actually reviewed. The response time is unaffected
by review. Every returned suggestion is recorded (SuggestionRecord).

## `POST /tankoubons/ai-group-suggestions/review`

Request: `{ "archive_ids": [...], "existing_tankoubon_id": null }`.

Response `200` (the review is wrapped, so the same `Review` shape is used here and inline on a
suggestion):

```json
{ "suggestion_id": "…",
  "review": { "status": "reviewed",
              "verdicts": [{ "archive_id": "a1", "keep": true, "reason": "…" }],
              "judged": ["a1", "a2"], "prompt_version": "v1" } }
```

Empty `verdicts`/`judged` and a null `reason` are omitted.

`status` is one of `reviewed`, `repaired`, `settled`, `fell_back`, `not_configured`. A failure is
never an HTTP error: `fell_back` with `reason` naming the failure class, so the card always renders.
`400` only for an empty member list. Members that no longer exist are dropped before review; if fewer
than two remain the status is `fell_back` with reason `members_changed`.

## `POST /tankoubons/ai-group-suggestions/labels`

Recorded after the user creates a suggestion:

```json
{ "archive_ids": [...], "existing_tankoubon_id": null,
  "kept": ["a1", "a2"], "unticked": ["a3"], "reviewer_rejected_kept": ["a2"] }
```

The suggestion id is derived server-side from `archive_ids` + `existing_tankoubon_id`; `kept` and
`unticked` must be members of `archive_ids` (`400` otherwise).

Writes ticked–ticked positives, ticked–unticked negatives, and one disagreement per
`reviewer_rejected_kept` id. Members of an existing tankoubon count as ticked. `204`.

## `POST /tankoubons/ai-group-suggestions/ignore` (existing, extended)

Additionally records every member pair as a negative label (`source: dismissed`). Response unchanged.

## `GET /tankoubons/ai-group-suggestions/labels`

`{ "positive": n, "negative": n, "disagreements": n }` — counts only, for the settings UI and the
evaluation's sanity check.

## `DELETE /tankoubons/ai-group-suggestions/labels`

Deletes all labels and disagreements (FR-017). `204`.
