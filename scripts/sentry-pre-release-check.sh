#!/usr/bin/env bash
# Pre-release diagnostics check (#285): what the tracker collected since the last release, read
# straight from rustrak's SQLite database over SSH, read-only.
#
#   scripts/sentry-pre-release-check.sh [--since <git-ref|date>] [--threshold N]
#
# Reports, per project (lanrurugi-dev / lanrurugi-prod):
#   1. issues first seen since then — each one fixed, accepted, or written down before release;
#   2. spans and transactions grouped by op + description — a group far above the rest (a request
#      tracing itself, a query refetching in a loop) is marked and must be explained.
#
# Environment: RUSTRAK_SSH (default `truenas`), RUSTRAK_DB (default the NAS path below).
set -euo pipefail

SSH_HOST="${RUSTRAK_SSH:-truenas}"
DB="${RUSTRAK_DB:-/mnt/tank/app-settings/rustrak/data/rustrak.db}"
SINCE=""
THRESHOLD=1000

while [[ $# -gt 0 ]]; do
  case "$1" in
    --since) SINCE="$2"; shift 2 ;;
    --threshold) THRESHOLD="$2"; shift 2 ;;
    -h|--help) sed -n '2,14p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

# Default: the commit date of the newest release tag reachable from HEAD.
if [[ -z "$SINCE" ]]; then
  SINCE="$(git describe --tags --abbrev=0 2>/dev/null || true)"
  [[ -n "$SINCE" ]] || { echo "no release tag found; pass --since" >&2; exit 2; }
fi
if git rev-parse -q --verify "$SINCE^{commit}" >/dev/null 2>&1; then
  label="$SINCE"
  SINCE="$(git log -1 --format=%cI "$SINCE")"
else
  label="$SINCE"
fi
# rustrak stores UTC ISO-8601 text, so a UTC prefix compares correctly as a string.
SINCE_UTC="$(date -u -d "$SINCE" +%Y-%m-%dT%H:%M:%S)"
[[ "$THRESHOLD" =~ ^[0-9]+$ ]] || { echo "--threshold must be a number" >&2; exit 2; }

echo "Diagnostics since ${label} (${SINCE_UTC}Z), threshold ${THRESHOLD}"

query() {
  # The SQL goes over stdin so no shell on either side re-quotes it.
  ssh "$SSH_HOST" "sudo sqlite3 -readonly -header -column 'file:${DB}?mode=ro'" <<<"$1"
}

query "
.print
.print == New issues (first seen since the last release) ==
SELECT p.slug AS project, i.numeric_id AS id, i.level, i.status,
       i.digested_event_count AS events, substr(i.first_seen, 1, 16) AS first_seen,
       substr(i.calculated_type || ': ' || i.calculated_value, 1, 90) AS title
FROM issues i JOIN projects p ON p.id = i.project_id
WHERE i.first_seen >= '${SINCE_UTC}'
ORDER BY p.slug, i.digested_event_count DESC;

.print
.print == Spans by op + description (top 15 per project; ! = over threshold) ==
SELECT project, flag, n, op, description FROM (
  SELECT p.slug AS project, s.op, substr(s.description, 1, 70) AS description, count(*) AS n,
         CASE WHEN count(*) > ${THRESHOLD} THEN '!' ELSE '' END AS flag,
         row_number() OVER (PARTITION BY p.slug ORDER BY count(*) DESC) AS rank
  FROM spans s JOIN projects p ON p.id = s.project_id
  WHERE s.start_timestamp >= '${SINCE_UTC}'
  GROUP BY p.slug, s.op, substr(s.description, 1, 70)
) WHERE rank <= 15 ORDER BY project, n DESC;

.print
.print == Transactions by name (top 15 per project; ! = over threshold) ==
SELECT project, flag, n, op, name FROM (
  SELECT p.slug AS project, t.op, substr(t.transaction_name, 1, 70) AS name, count(*) AS n,
         CASE WHEN count(*) > ${THRESHOLD} THEN '!' ELSE '' END AS flag,
         row_number() OVER (PARTITION BY p.slug ORDER BY count(*) DESC) AS rank
  FROM transactions t JOIN projects p ON p.id = t.project_id
  WHERE t.timestamp >= '${SINCE_UTC}'
  GROUP BY p.slug, t.op, substr(t.transaction_name, 1, 70)
) WHERE rank <= 15 ORDER BY project, n DESC;
"
