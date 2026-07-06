# Contract: Benchmark Comparison Report (US8, FR-020–022, SC-011)

Shape returned by `GET /bench/{reportid}` (see `contracts/rest-api.md`) and produced by
`bench/compare`.

```json
{
  "report_id": "string",
  "generated_at": "ISO-8601 timestamp",
  "library_scale": { "archive_count": 100000, "total_size_bytes": 0 },
  "hardware": { "cpu_cores": 0, "description": "string, user-supplied/free text" },
  "operations": [
    {
      "operation": "full_library_scan_ingestion" | "duplicate_repair_reindex",
      "legacy": { "wall_clock_seconds": 0.0, "throughput_archives_per_second": 0.0 },
      "new": { "wall_clock_seconds": 0.0, "throughput_archives_per_second": 0.0 },
      "speedup_factor": 0.0
    }
  ]
}
```

**Rules**:
- `operations` MUST include at least `full_library_scan_ingestion` and
  `duplicate_repair_reindex` (FR-020).
- The report is produced from an actual run against both systems on the same hardware/library
  copy (FR-021) — it is not a projection or estimate.
- `speedup_factor` MUST NOT be presented as a fixed promised number anywhere outside this
  generated report (spec Assumptions: "not a claim about any specific numeric speedup factor").
- If run on a single-core host, `operations[].new.throughput_archives_per_second` MAY be close to
  `legacy`'s value — the report MUST still be produced (edge case from spec.md's Edge Cases), not
  suppressed or errored.
