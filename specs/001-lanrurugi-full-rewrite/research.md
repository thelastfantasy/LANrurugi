# Phase 0 Research: LANrurugi — Full Rewrite (Phase 1)

Each entry: Decision / Rationale / Alternatives considered. Facts about legacy behavior below were
verified by reading the actual LANraragi source (`~/LANraragi`), per constitution's "verify
against source" workflow rule — not assumed from memory.

## 1. Archive-identity hash function: keep SHA-1, don't switch to BLAKE3, for the size-aware ID

**Decision**: The new default "size-aware" archive ID (`hash(first 512KB ++ u64 BE file size)`,
fixed by constitution) uses **SHA-1** as the hash function, producing the same 40-hex-character
digest length as the legacy algorithm — not BLAKE3's default 64-hex-character (32-byte) output.

**Rationale**: Reading `lib/LANraragi/Model/Backup.pm` confirms archive IDs are enumerated
elsewhere in the codebase by a **40-character Redis key-glob pattern**
(`????????????????????????????????????????` — see `build_backup_JSON`), and this length
assumption likely recurs in other key-scanning code paths (stats, database maintenance). Changing
the digest length would break that pattern-matching for *every* archive, old and new alike, unless
every such glob is found and rewritten. Keeping SHA-1 (with the new size-aware input) preserves
the 40-char length for both legacy and new IDs, so existing length-based key enumeration keeps
working unmodified. Since the actual bottleneck for hashing during a scan is disk I/O, not hash
compute time (established in prior design discussion), SHA-1 vs BLAKE3 has no measurable
performance impact on this specific 512KB-sample operation — the real concurrency win (US8) comes
from parallelizing *how many files* are hashed at once (rayon), not from a faster hash primitive
per file.

**Alternatives considered**:
- BLAKE3, truncated to 160 bits (20 bytes) to force a 40-hex-char digest — technically valid
  (BLAKE3 supports extendable/truncated output) but adds an audit burden and a "why is this
  truncated" question for zero real benefit, since the bottleneck isn't hash speed.
- BLAKE3 full-length (64 hex chars) — rejected: would require finding and fixing every
  fixed-length key-glob in the codebase, which is exactly the kind of silent-breakage risk
  Principle I exists to prevent.
- Full-file BLAKE3 hashing (abandoning the 512KB-sample approach) — out of scope here; the
  512KB-sample-plus-size design was already fixed by prior clarification and the constitution.

## 2. Redis client crate

**Decision**: `redis` crate (the de facto standard async Rust Redis client) with `deadpool-redis`
for connection pooling, both used via `tokio`.

**Rationale**: Widely used, actively maintained, supports the full Redis command surface the
legacy Perl `Redis` client relies on (hashes, sorted sets, sets, pub/sub not currently needed).
`deadpool-redis` gives a standard async pool pattern that composes cleanly with Axum's shared
state.

**Alternatives considered**: `fred` (also solid, more built-in cluster/reconnect ergonomics) —
not chosen because this deployment is explicitly single-instance/non-clustered (per the spec's
scale Clarification), so `fred`'s extra clustering sophistication isn't needed; `redis` + a manual
pool — deadpool is simpler and already the common pairing.

## 3. Archive format handling (zip/rar/7z/pdf/epub)

**Decision**: `zip` crate for ZIP archives (the majority case). RAR extraction shells out to the
`unrar` binary (never reimplemented — RAR's format/license make a native Rust reimplementation
both legally and practically unwise, and this matches what the legacy system effectively does by
depending on external tools too). 7z shells out to `7z`/`7zz`. PDF/EPUB handled by dedicated
crates where mature ones exist, falling back to shelling out otherwise — exact crate selection is
an implementation detail for the tasks phase, not a spec-level decision.

**Rationale**: Matches the reasoning already established in prior design discussion: RAR/7z are
not worth reimplementing from scratch, and shelling out is the pragmatic, well-precedented choice
(the legacy Perl system does the same for some formats).

**Alternatives considered**: Pure-Rust RAR decompression — no mature, legally unencumbered crate
exists; rejected.

## 4. Thumbnailing / image processing

**Decision**: The `image` crate for decode/resize, invoked as CPU-bound work dispatched via
`rayon` + `tokio::task::spawn_blocking` (never inline in an async handler).

**Rationale**: Standard, well-maintained pure-Rust imaging crate; avoids a native dependency
(e.g. libvips bindings) for the common case, keeping the Docker image lean per Principle III.

**Alternatives considered**: `libvips` bindings — faster for some workloads but adds a native
system dependency to the Debian slim image; deferred unless benchmarking (US8-adjacent, but for
thumbnailing specifically, not the two operations FR-020 mandates) shows `image` insufficient.

## 5. Concurrency bridging: rayon ↔ tokio

**Decision**: A single process-wide `rayon::ThreadPool` sized to available cores (default rayon
behavior), invoked from async code exclusively via `tokio::task::spawn_blocking(move || { ... })`
wrapping a `rayon::scope`/parallel-iterator call, with the result sent back via the `spawn_blocking`
join handle. No CPU-bound work (hashing, image decode/resize) ever runs directly inside an `async
fn` body on a Tokio worker thread.

**Rationale**: This is the standard, documented-safe pattern for mixing rayon and tokio (rayon's
own thread pool is separate from Tokio's worker threads; `spawn_blocking` moves the blocking/CPU-
heavy closure onto Tokio's dedicated blocking-thread pool so it can't stall the async reactor that
serves concurrent HTTP requests). This directly implements constitution Principle III's
concurrency-model requirement.

**Alternatives considered**: Running rayon parallel iterators directly inside async handlers —
rejected, this blocks the calling Tokio worker thread and would degrade request latency for all
concurrent users during any bulk operation (exactly the failure mode Principle III's bridging rule
exists to prevent).

## 6. File watching (Shinobu replacement)

**Decision**: `notify` crate for cross-platform filesystem events, wrapped in a `tokio::spawn`
task with a debounce window (3–5s, matching the legacy Shinobu's existing debounce behavior found
in `lib/Shinobu.pm`) before triggering ingestion, and a per-file `tokio::time::timeout` (~30s) so a
pathological file can't stall the ingestion pipeline — both behaviors already established in prior
design discussion and consistent with FR-006/FR-007.

**Rationale**: `notify` is the standard cross-platform (Linux/macOS/Windows) file-watching crate
in the Rust ecosystem; matching the legacy debounce/timeout numbers avoids introducing a
regression in how "still being written" files are handled (FR-006).

**Alternatives considered**: Polling-based scanning only (no `notify`) — rejected as a regression
versus the legacy event-driven Shinobu.

## 7. Plugin subprocess protocol (Deno)

**Decision**: A persistent pool of long-lived Deno subprocesses (sized modestly, not one per
core — plugin work is I/O-bound network calls to metadata sites, not CPU-bound), each running a
small dispatcher script that `import()`s the requested plugin module on demand, communicating with
the Rust host via newline-delimited JSON over stdin/stdout (a minimal JSON-RPC-shaped protocol:
request has plugin name + method + args; response has result or error). Declared permissions
(`--allow-net=<hosts>`, etc.) are read from a plugin manifest and passed as Deno CLI flags when
that plugin's subprocess/pool is started.

**Rationale**: Matches constitution Principle IV exactly (subprocess isolation, permission model,
persistent worker pool over one-shot spawns) and the FR-012–FR-014 requirements (enrichment,
isolated failure, denied-by-default capabilities) from the spec.

**Alternatives considered**: One Deno process per plugin invocation — rejected per Principle IV's
"SHOULD use a persistent worker pool" guidance, since plugin invocations happen frequently enough
during bulk enrichment that per-call cold starts would be noticeable.

## 8. Search engine port

**Decision**: Port the legacy Redis-based search model directly rather than introducing a new
search index technology: sorted sets for title ordering (mirrors `LRR_TITLES`), tag-based
filtering via Redis set operations, and a search-results cache keyed by the same
filter/sort/flags tuple the legacy `check_cache`/`do_search` (`lib/LANraragi/Model/Search.pm`)
already uses. The query-string grammar (namespace:tag, wildcards, boolean combination) is
reimplemented with a parser combinator crate (`nom` or `pest` — final choice deferred to the
tasks phase; both are viable, well-established options for this).

**Rationale**: Reading `Model/Search.pm` confirms the legacy engine already does a full DB parse
on cache miss and caches by a composite key of the search parameters; porting this behavior
preserves FR-013/FR-014's "existing search syntax returns equivalent results" requirement (US3)
without inventing new search semantics users would need to relearn.

**Alternatives considered**: A dedicated search engine (e.g. Tantivy) — rejected for Phase 1;
would be a bigger behavioral and operational change than "port existing search," and isn't
required to hit SC-008's scale target (~100k archives) with Redis-based filtering plus caching.

## 9. Backup/export format

**Decision**: The backup/export JSON shape mirrors what `build_backup_JSON` in
`lib/LANraragi/Model/Backup.pm` already produces: a top-level object with `categories` (from
`SET_*` keys: id, name, saved search, archive list), `tankoubons` (id, name, summary, tags,
ordered archive list), `stamps` (id, content, position, archive_id), and `archives` (per-ID
title/tags/summary/thumbhash), keyed by the same archive ID scheme so restore can re-attach
metadata to the matching archive.

**Rationale**: This is a direct "verify against source" finding — reusing the legacy shape means
FR-008/FR-009 (trigger backup, restore onto a fresh instance) can be satisfied without inventing a
new schema, and keeps the door open for cross-compatibility (a LANraragi backup could plausibly be
importable, though that's not an FR requirement, just a nice side effect of shape reuse).

**Alternatives considered**: A new, LANrurugi-specific backup schema — rejected as unnecessary
complexity; nothing about the legacy shape conflicts with any Phase 1 requirement.

## 10. Frontend i18n (UI localization, US7)

**Decision**: Frontend uses a standard React i18n library (e.g. `react-i18next`) with JSON
translation resources; legacy translation content lives in `locales/template/*.po` (gettext
format: `en`, `ja`, `zh`, `zh_Hant`, `ko`, `fr`, `de`, `es`, `it`, `pt`, `vi`, `id`, `nb_NO`, `as` —
14 languages, verified by listing the actual directory). A one-time conversion step transforms
`.po` → i18next-shaped JSON per language during the porting task, rather than shipping a `.po`
parser/runtime in the frontend.

**Rationale**: `.po`/gettext is a backend-oriented i18n format not natively consumed by the
React/i18next ecosystem; converting once at build/port time is simpler and lighter at runtime than
shipping gettext parsing to the browser. This satisfies FR-018/FR-019 (language selection,
fallback-to-English for missing strings) using idiomatic frontend tooling.

**Alternatives considered**: Parsing `.po` files directly in the browser — rejected, unnecessary
runtime cost and dependency for a one-time content-format conversion problem.

## 11. Benchmark harness design (US8)

**Decision**: Three pieces, per the `bench/` structure in plan.md: (a) a synthetic-library
generator producing a reproducible ~100k-archive test library at the SC-008 target scale
(configurable smaller sizes for fast local runs); (b) `criterion`-based in-process Rust
microbenchmarks for isolated operations (hashing throughput, thumbnail decode/resize); (c) a
`compare/` orchestration harness that runs full library scan/ingestion and the duplicate-repair
reindex against *both* a running legacy LANraragi instance and the new LANrurugi binary on the
same hardware/library copy, capturing wall-clock time and throughput, and emitting a comparison
report (plain-text/Markdown table, per FR-020/FR-021).

**Rationale**: Directly implements US8/FR-020–022/SC-011. Separating microbenchmarks (fast,
run-every-CI) from the cross-system comparison (slower, needs a legacy instance available) matches
how these are actually used — `criterion` benches run continuously during development, the full
cross-system comparison is run less frequently (e.g. before a release) since it requires standing
up the legacy Perl system too.

**Alternatives considered**: Only `criterion` microbenchmarks, no cross-system harness — rejected;
microbenchmarks alone can't produce the "faster than the previous system" claim SC-011 and
FR-021 require, since that needs an actual side-by-side run of both systems.

## 12. API contract source

**Decision**: `contracts/` for this plan is derived directly from the legacy
`tools/openapi.yaml` (verified present and read in this project), covering the endpoint groups
that exist there today (archives, search, categories, tankoubons, plugins, shinobu, minion,
database/backup, opds, stamps), reproduced as the Phase 1 contract with additive-only extensions
(e.g. an explicit rebuild-index endpoint, a benchmark-trigger endpoint) appended rather than
altering existing paths, per constitution Principle II.

**Rationale**: This is the actual existing contract third-party clients depend on (US3); the
additive-only rule keeps FR-014 satisfied.

**Alternatives considered**: Designing a fresh REST contract "the way we'd do it today" — rejected
for Phase 1 per Principle II; would break FR-013's existing-client compatibility requirement.

## 13. CLI structure

**Decision**: `clap`-based subcommands on the single `lanrurugi-server` binary: `lanrurugi serve`
(the normal running mode — Axum server + scanner + plugin pool, all in one Tokio runtime),
`lanrurugi rebuild-index` (US6's duplicate-repair/reindex tool), `lanrurugi bench` (invokes the
`bench/compare` harness).

**Rationale**: Keeps the "one binary" promise of Principle III literally true — operational
tooling is subcommands of the same artifact, not separate binaries to build/ship/version
independently.

**Alternatives considered**: Separate binaries per tool (`lanrurugi-rebuild`, `lanrurugi-bench`) —
rejected as unnecessary proliferation for what are fundamentally administrative modes of the same
program.
