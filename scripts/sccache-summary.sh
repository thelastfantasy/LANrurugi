#!/usr/bin/env bash
# Reports sccache's hit rate and estimated time saved for this CI run, to both the step log and
# (when running under GitHub Actions) $GITHUB_STEP_SUMMARY. Run once at the end of a job that
# built Rust code with RUSTC_WRAPPER=sccache.
set -euo pipefail

stats="$(sccache --show-stats --stats-format json)"

hits=$(echo "$stats" | jq '[.stats.cache_hits.counts[]] | add // 0')
misses=$(echo "$stats" | jq '[.stats.cache_misses.counts[]] | add // 0')
requests=$(echo "$stats" | jq '.stats.compile_requests')
compilations=$(echo "$stats" | jq '.stats.compilations')
cache_write_secs=$(echo "$stats" | jq '(.stats.cache_write_duration.secs // 0) + (.stats.cache_write_duration.nanos // 0) / 1000000000')
compiler_write_secs=$(echo "$stats" | jq '(.stats.compiler_write_duration.secs // 0) + (.stats.compiler_write_duration.nanos // 0) / 1000000000')
cache_read_secs=$(echo "$stats" | jq '(.stats.cache_read_hit_duration.secs // 0) + (.stats.cache_read_hit_duration.nanos // 0) / 1000000000')
cache_size=$(echo "$stats" | jq -r '.cache_size')
max_cache_size=$(echo "$stats" | jq -r '.max_cache_size')

total=$((hits + misses))
if [ "$total" -gt 0 ]; then
  hit_rate=$(awk "BEGIN { printf \"%.1f\", 100 * $hits / $total }")
else
  hit_rate="0.0"
fi

# Estimated wall-clock time avoided: each cache hit skipped a real compile whose average cost is
# derived from *this run's own* actual (uncached) compiles, so it's a same-run, same-hardware
# estimate rather than a guess pulled from unrelated history.
if [ "$compilations" -gt 0 ]; then
  avg_compile_secs=$(awk "BEGIN { printf \"%.3f\", $compiler_write_secs / $compilations }")
  estimated_saved_secs=$(awk "BEGIN { printf \"%.1f\", $hits * $avg_compile_secs - $cache_read_secs }")
else
  avg_compile_secs="0.000"
  estimated_saved_secs="0.0"
fi

cache_size_mib=$(awk "BEGIN { printf \"%.0f\", $cache_size / 1048576 }")
max_cache_size_gib=$(awk "BEGIN { printf \"%.0f\", $max_cache_size / 1073741824 }")

summary="### sccache stats — this run

| Metric | Value |
|---|---|
| Hit rate | ${hit_rate}% (${hits}/${total}) |
| Compile requests | ${requests} |
| Real compiles (misses) | ${compilations} |
| Avg. real compile time | ${avg_compile_secs}s |
| Est. time saved by cache hits | ${estimated_saved_secs}s |
| Cache size | ${cache_size_mib} MiB / ${max_cache_size_gib} GiB |
"

echo "$summary"
if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
  echo "$summary" >> "$GITHUB_STEP_SUMMARY"
fi
