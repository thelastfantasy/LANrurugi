# Contract: `lanrurugi-server eval-grouping`

```sh
lanrurugi-server eval-grouping [--redis-url URL] [--candidate settings.json] [--review off|cached|live]
```

- Reads labels; if none, prints that there is nothing to measure and exits 0.
- Candidate pool = every archive appearing in a label that still exists and has a cached title
  vector (the rest are counted as skipped). Existing tankoubons are not added as nodes: labels are
  archive pairs, and "add to existing" labels already pair new members with the tankoubon's own.
- Runs the production grouping code with `GroupingSettings::default()` ("current") and, if given,
  the candidate (fields not present default to current).
- `--review`: `off` (default) measures the local stage only; `cached` applies cached reviews and
  treats uncached groups as unreviewed; `live` may call the provider for uncached groups and caches
  the results like the endpoint does.
- Pair metrics over labelled pairs only:
  - precision = labelled-positive pairs proposed together ÷ labelled pairs proposed together;
  - recall = labelled-positive pairs proposed together ÷ labelled-positive pairs.
  "Proposed together" after review means both members kept.
- Output: one table per variant and stage plus the deltas, label counts, and the number of groups
  reviewed by the provider during the run (each is one call, or two with a repair).
- Read-only for library data, labels and suggestions; `live` writes review-cache entries only.
