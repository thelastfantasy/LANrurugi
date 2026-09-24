# Research: Automated UI Test Coverage

All version numbers below were checked live against npm/crates.io/apt at plan time (2026-07-18),
per constitution's "latest stable release, verified at implementation time — not a remembered or
copied-over version" rule — none are carried over unverified from `~/jellyfin-suite`'s own pins or
an earlier session's memory.

## 1. Vitest + React Testing Library (unit-level, no browser)

**Decision**: `vitest@4.1.10`, `@testing-library/react@16.3.2` + `@testing-library/dom` (explicit
peer dependency as of RTL v16) + `@testing-library/jest-dom` (imported via its `/vitest` subpath
entry point in the test setup file, not the default entry), `jsdom@29.1.1` as `test.environment`.
Do **not** add `@vitest/browser`.

**Rationale**: Vitest 4.1 added native Vite 8 support and now reuses the project's already-installed
Vite version instead of vendoring its own, so it consumes the same `vite.config.ts`/
`@tailwindcss/vite` setup already in `apps/frontend/` with no separate Vitest-specific Tailwind
config — only a `test: {...}` block needs to be layered into the same config (or a
`vitest.config.ts` that imports the app's Vite config, matching `~/jellyfin-suite`'s own pattern of
a separate `vitest.config.ts` rooted at the repo root). RTL 16.x added React 19 compatibility; since
v16, `@testing-library/dom` and `@types/react-dom` moved to peer dependencies and must be installed
explicitly rather than pulled in transitively. `@testing-library/jest-dom`'s matchers
(`toBeInTheDocument`, `toHaveTextContent`, etc.) are not part of Vitest's built-in Jest-compatible
`expect` — the package must still be installed and its `/vitest` entry point imported in test
setup. `@vitest/browser` runs test files inside a real browser tab (via a Playwright/WebdriverIO
provider) for layout/CSS/real-event fidelity — explicitly the wrong layer here since Playwright
(below) already covers real-browser rendering; Vitest's own docs frame the split as
jsdom-unit-tests-for-logic + Playwright-for-cross-browser-E2E, matching this project's stated
scope (hooks, pure functions, state machines — spec FR-001).

**Alternatives considered**: `@vitest/browser` for component-in-browser tests (rejected as
redundant given Playwright already covers real-browser rendering). `happy-dom` as a lighter/faster
jsdom alternative (viable, but jsdom remains the more standards-complete default and is what
Vitest's own getting-started guide uses, plus it's what `~/jellyfin-suite` already uses). A
`jest-dom` fork like `vitest-dom` (rejected — unmaintained for ~3 years vs. `jest-dom`'s actively
maintained `/vitest` export).

## 2. Playwright (Chromium + Firefox, retry, failure artifacts)

**Decision**: `@playwright/test@1.61.1`. Config:
```ts
projects: [
  { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  { name: 'firefox', use: { ...devices['Desktop Firefox'] } },
], // WebKit project omitted entirely — out of scope per spec Clarifications
retries: process.env.CI ? 1 : 0,
use: { screenshot: 'only-on-failure', trace: 'on-first-retry' },
```

**Rationale**: Playwright's own recommended CI defaults are exactly `screenshot: 'only-on-failure'`
+ `trace: 'on-first-retry'` — zero overhead on passing runs, a full replayable trace only when a
test is actually retried. On `retries`: confirmed via official docs that `retries: N` means N
retries **in addition to** the initial run (N+1 total attempts); `testInfo.retry` is 0-indexed. So
`retries: 1` gives exactly spec FR-012's semantics: one initial attempt, one retry, reported
"Failed" only if both the original and the retry fail (a fail-then-pass sequence is reported as
"Flaky," not "Failed" — Playwright already distinguishes these, satisfying "only a second
consecutive failure counts as a real failure" without extra logic). Locally (`retries: 0` when
`CI` is unset), `trace: 'retain-on-failure'` is the documented alternative for capturing
unconditionally-recorded-but-only-kept-on-failure traces without needing a retry to trigger one —
worth using in local dev config since local runs default to no retries.

**Alternatives considered**: Including WebKit as a third project (rejected — out of scope per
spec's Chromium+Firefox Clarification). `retries: 2` (rejected — spec explicitly wants exactly one
retry).

## 3. Playwright + multi-service orchestration (frontend + Rust backend + Redis)

**Decision**: Use `webServer` (as an **array**) for the simple, single-command, HTTP/port-pollable
processes — starting `redis-server`, the compiled `lanrurugi-server` binary, and the frontend
preview server as three array entries, each with its own `command` + readiness probe (port-open or
non-5xx HTTP check) + `reuseExistingServer: !process.env.CI`. Use `globalSetup`/`globalTeardown`
for anything requiring sequencing or non-trivial teardown beyond "run this command and wait for a
port" — in particular, flushing/seeding Redis state and explicitly polling the backend's health
endpoint before tests start (see caveat below), plus fixture-archive upload if any suite-wide
seed data is needed.

**Rationale**: `webServer`'s array form is directly usable for Redis + Rust backend + frontend
preview as three independent entries with per-service readiness diagnostics (rather than one
opaque combined health check). However, `webServer` entries have no built-in ordering/dependency
guarantee between array entries and teardown is a bare process-kill — a poor fit for
"Redis must be up before the Rust backend starts" sequencing or "flush Redis between suites."
`globalSetup`/`globalTeardown` (plain async functions) or a dependency "setup project" are the
better fit for that. **Known caveat, confirmed via an open Playwright GitHub issue**: `globalSetup`
is not guaranteed to run only after `webServer` readiness — if global setup needs to reach the
backend, it must still poll for readiness itself even with `webServer` configured, rather than
assuming ordering.

**Alternatives considered**: A single `webServer` command shelling out to a combined
startup script for all three services (rejected — hides per-service failures behind one opaque
readiness check). Pure `globalSetup`/`globalTeardown` for the frontend server too (rejected —
reinvents `webServer`'s built-in readiness-polling and `reuseExistingServer` for no benefit).

## 4. Parallel workers + isolated Redis per worker (FR-014)

**Decision**: Each worker starts its own `redis-server` process on its own port (derived from
`testInfo.parallelIndex`, e.g. `6380 + parallelIndex`), and its own `lanrurugi-server` backend
process pointed at that instance, rather than sharing one Redis instance partitioned by logical
database number.

**Rationale**: The originally-considered approach — a single shared Redis instance, workers
partitioned via `SELECT 0`-`15` keyed by `parallelIndex` — turned out to be unusable once the
backend's actual Redis usage was checked directly: `RedisDbs::connect`
(`crates/lanrurugi-storage/src/redis.rs`) takes one *bare* base URL (no db index) and internally
opens **five** fixed-offset logical databases relative to it (`archive`=+0, `minion`=+1,
`config`=+2, `search`=+3, `metrics`=+4) — a single backend process's Redis footprint is 5 logical
DBs, not 1. Against Redis's default 16-DB ceiling, that allows at most 3 non-overlapping workers
(`0-4`, `5-9`, `10-14`), which is both an awkward fit and silently breaks the moment a 4th worker
is requested (this was caught by actually starting a real backend process against the naively
`/0`-suffixed URL during implementation — it failed immediately with "Invalid database number",
not a bug that would have been caught by inspecting `research.md`'s original text alone). A
separate Redis instance per worker sidesteps the DB-count ceiling entirely: each worker's `1..5`
five-DB footprint lives in its own Redis process, so there is no shared 16-DB budget to divide up
in the first place, and it scales past 16 workers if that's ever needed. This was noted in the
original decision as "the scaling path, not the initial approach" — it turned out to be the only
correct approach once the backend's real multi-DB-per-process shape was accounted for.

**Alternatives considered**: `SELECT`-based single-shared-instance partitioning keyed by
`parallelIndex` (this was the original decision — rejected after direct verification against the
backend's actual `RedisDbs::connect` 5-DB-per-process behavior, not a hypothetical). Keying off
`workerIndex` directly (rejected regardless of DB-vs-port approach — unbounded, would eventually
misbehave under worker restarts). A shared single DB with per-test key-prefixing only (rejected —
this project's own five logical databases are legacy-compatibility-mandated key namespacing
already; layering a second, ad-hoc namespacing scheme on top for test isolation would fight the
existing one rather than compose with it).

## 5. Multi-volume and encrypted archive fixture generation

**Decision**: Use the `7z` CLI (Debian's `p7zip-full` package, confirmed present, v26.00) to
generate both multi-volume and encrypted fixtures, for both the 7z and zip container formats — not
a Rust crate.

**Rationale**: Verified live: `7z a -v1k archive.7z file` produces genuine split volumes
(`archive.7z.001`, `.002`, …). `7z a -p<pass> -mhe=on archive.7z file` produces AES-256-encrypted
7z with both content *and* header/filenames encrypted (`-mhe=on`). `7z a -tzip -p<pass>
archive.zip file` produces a password-protected (ZipCrypto) zip; `-tzip -v<size>` produces
multi-volume zip too — one binary covers both formats and both higher-risk shapes. `sevenz-rust2`
v0.21.3 (Apache-2.0, the actively-maintained fork already in this project's Cargo registry cache)
does support writing AES-encrypted 7z (`compress_to_path_encrypted`, `AesEncoderOptions`) — a valid
pure-Rust alternative for the *encrypted, non-multivolume* 7z fixture specifically — but a full
source grep found **zero** multi-volume/split support anywhere in the crate, so it cannot cover
FR-010's multi-volume requirement on its own. Using the `7z` CLI for both shapes (rather than
splitting generation between a CLI and a crate) keeps fixture generation on one consistent tool.
RAR has no apt-available writer at all (proprietary format) and no Rust crate writes it — see §6.

**Alternatives considered**: `sevenz-rust2` in-process for the encrypted-7z fixture only (viable,
rejected in favor of one consistent generation path since the CLI already covers every other
shape). Hand-rolling multi-volume/encrypted byte structures directly (rejected — needlessly
fragile compared to a real, already-available CLI; hand-rolling was only necessary for the
non-UTF-8-flagged zip case in §6.1 below, where no tool produces that specific legacy shape).

### 5.1. lzh/lha fixture generation

**Decision**: Hand-build `sample.lzh`/`sample.lha` as a minimal valid LHA/LZH level-1 archive
(method `-lh0-`, i.e. stored/uncompressed — no LZSS+Huffman codec needs implementing), the same
"construct the container format's bytes directly" approach already used for the non-UTF-8-flagged
zip case in §5.2 below. Verify the result against `delharc` (an independent, actively-maintained
pure-Rust LHA/LZH reader — not the same code path as whatever generated it) and, more importantly,
against this project's own `lanrurugi-scanner::archive_format::list_pages` (the real libarchive
read path the fixture actually needs to satisfy) before accepting it as a fixture.

**Rationale**: Neither `7z` nor Debian's `lhasa` package can *create* an lzh/lha archive on this
platform — verified live, not assumed: `7z a sample.lzh ...` fails with `E_NOTIMPL`, and `lhasa`'s
own `lha` command has no `a` (add/create) subcommand at all — it is a decompressor only, despite
the package description ("lzh archive decompressor"), and no other FOSS lzh/lha writer exists in
Debian's repos or as a mature Rust crate (the one candidate, `oxiarc-archive` v0.3.6, is too new/
unproven to depend on for fixture generation). This is the same situation as RAR (§6): no FOSS
writer exists. Unlike RAR, however, no pre-made redistributable fixture set (comparable to the
`unrar` crate's bundled files or `ssokolow/rar-test-files`) is known to exist for lzh/lha either, so
"source a pre-made fixture" isn't available as a fallback the way it is for RAR — hand-building the
container bytes directly (already the established pattern for the non-UTF-8-flagged zip case) is
the only remaining option. The level-1 header format is simple and fully documented (a fixed
19-byte base header + filename + CRC-16 + OS-ID + a terminating 2-byte "next extended header size"
field of `0x0000`); `-lh0-` avoids needing to implement LHA's actual compression codec, matching
`sample.7z`/`sample.zip`'s own use of default/light compression settings rather than exercising
compression-algorithm correctness (this project's own fixtures test ingestion/reading, not
recompression). One real construction pitfall worth recording for whoever regenerates this fixture
later: the level-1 header-size byte does **not** count the trailing 2-byte "next extended header
size" field itself, but a conforming parser's own running byte-length counter already includes the
header-size/checksum bytes' own 2 bytes by the time it reaches the header-size check — verified by
patching `delharc`'s parser with debug tracing when a byte-count-off-by-2 mismatch first surfaced,
not by re-deriving the spec from documentation alone.

**Alternatives considered**: `oxiarc-archive` (rejected — v0.3.6, unproven, adds a real dependency
for a one-time fixture-generation need better served by a self-contained script). Skipping lzh/lha
fixture coverage entirely (rejected — defeats FR-009's requirement that every
`lanrurugi-scanner`-supported format have fixture coverage, and this project's own scanner code
comments note libarchive's lzh/lha decode correctness has never actually been exercised by a real
fixture before this feature).

### 5.2. The one shape no current tool produces: non-UTF-8-flagged zip

Carried over from this project's own prior CJK-mojibake bug fix: no tool (including `7z`) produces
a zip entry with the UTF-8 general-purpose-bit-flag deliberately left *unset* while using a legacy
CJK encoding (Shift-JIS/GBK/etc.) — every modern zip tool sets that flag for non-ASCII names. That
fixture must continue to be hand-built via raw byte manipulation of the local-file-header/
central-directory/EOCD structure (as already done once for `crates/lanrurugi-scanner`'s own test
suite) — this is a one-off, already-solved case, not a gap this feature needs to re-solve.

## 6. RAR fixtures (unwritable via FOSS tooling)

**Decision**: Reuse the `unrar` crate's (v0.5.8, MIT/Apache-2.0) bundled test `.rar` files directly.
Do not attempt to generate new RAR fixtures in CI or as part of this feature's tooling.

**Rationale**: The `unrar` crate's own `data/` directory already ships exactly the shapes needed,
as tiny, redistribution-safe files bundled in a published crates.io package: `crypted.rar`
(password-protected), `archive.part1.rar` + `100M.part00002.rar` (multi-volume/split),
`unicode.rar` / `unicodefilename❤️.rar` (non-ASCII names — their actual bundled content is
Latin/symbol/emoji text, not genuine CJK; neither substitutes for this project's own CJK-mojibake
regression, which is why Regression Fixture #6's RAR extension (tasks.md) must reuse the
project's own hand-built CJK zip content inside a RAR wrapper rather than treating `unicode.rar` as
a CJK fixture), `solid.rar`. This is a solved problem in the wider ecosystem, not a gap unique to
this project:
[`ssokolow/rar-test-files`](https://github.com/ssokolow/rar-test-files) exists specifically because
its author bought a WinRAR license to generate RAR3/RAR5 fixtures (including `.cbr`) for testing an
`unrar`-invoking tool, then released them as CC0 — the accepted community pattern is "someone with
a licensed RAR-creation tool generates fixtures once, outside CI, and commits the resulting tiny
binaries," since no FOSS RAR-writer exists and generation can never be a CI-time step for this
format. If a project-specific RAR fixture is ever needed beyond what `unrar`'s bundled files or
`rar-test-files` already cover (e.g. embedding this project's own CJK mojibake scenario inside a
real RAR specifically, not just a zip/7z), that would be a one-time maintainer task using a
personally-licensed tool, checked in afterward — not part of this feature's automated tooling.

**Alternatives considered**: Generating project-specific RAR fixtures via a maintainer's licensed
tool up front (deferred — check whether `unrar`'s bundled `unicode.rar`/`rar-test-files` already
cover the needed shapes before spending that effort). Skipping RAR fixture coverage entirely
(rejected — defeats FR-009's format coverage requirement and would leave the exact RAR-format risk
area this project already found one real bug in — a broken `unrar-free` CLI syntax — untested).

## 7. Fixture directory placement (shared between Rust unit tests and Playwright E2E)

**Decision**: One shared directory at the repo root, `test-fixtures/archives/` (sibling to
`crates/` and `apps/`), used as the single source of truth by both layers. Move the existing
`crates/lanrurugi-scanner/tests-fixtures/cjk-names.7z` into it as part of this feature.

**Rationale**: No Playwright/E2E directory exists yet — this is a greenfield placement decision,
not a refactor of an established split. Both layers need bit-identical files (Rust via
`include_bytes!` at compile time; Playwright via `setInputFiles(path)` against a real HTTP upload
endpoint at runtime, since E2E tests upload through the real API, not by calling Rust functions
directly) — maintaining two copies risks silent drift with zero benefit. A single top-level shared
directory is reachable by relative path from any crate and matches Playwright's own convention of
a dedicated fixtures directory referenced by `path.join(__dirname, ...)`.

**Alternatives considered**: Keeping fixtures under `crates/lanrurugi-scanner/tests-fixtures/` with
Playwright reaching across via a relative `../../` path (rejected — buries a both-layers-need-it
asset inside one crate's directory, implying false single ownership). Separate copies per layer
(rejected — the explicit duplication/drift risk this decision exists to avoid).

## 8. Checked-in binary fixtures vs. generated at test time

**Decision**: Check fixtures directly into git as small binary files. No Git LFS. Do not generate
them via a CI setup script.

**Rationale**: Fixtures are a few hundred bytes to a few KB each (1-2 tiny placeholder images per
archive — real manga pages are not needed). Community LFS guidance centers on "large and/or
frequently-changing" (rule-of-thumb ~5-10MB+, or repos where binary churn from frequent edits
bloats clone size) as the point where LFS's complexity pays off; these fixtures are static
(created once, rarely touched) and orders of magnitude below that threshold — LFS also doesn't
compress objects the way native git blobs do, so for small compressible files plain git is often
*more* space-efficient. Generating fixtures via a CI setup script would reintroduce exactly the
tooling gaps documented in §5–6: no FOSS RAR writer exists at all, and multi-volume/encrypted
generation would make every environment that runs tests (CI *and* any contributor's local machine)
depend on `p7zip-full` being installed — checked-in binaries have zero such runtime dependency.

**Alternatives considered**: A generation script run once during CI/local setup (rejected as the
default — adds a `7z`-availability dependency everywhere tests run, and still can't produce RAR at
all, so it would need to special-case-fetch the checked-in RAR files anyway, undermining "one
consistent approach"). Git LFS (rejected — file sizes are far below where its overhead is
justified, and the project doesn't already have LFS configured).
