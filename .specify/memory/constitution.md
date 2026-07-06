<!--
Sync Impact Report
===================
Version change: 1.0.0 → 1.6.0 (MINOR — materially expanded, non-negotiable tooling/architecture
constraints)
Modified principles: III retitled "Resource-Conscious, Genuinely Concurrent Single-Process
Architecture" (was "Resource-Conscious, Single-Process Architecture") and expanded with a
concurrency-model bullet; no prior wording removed or weakened, purely additive.
Added sections/content (across six amendments, 2026-07-05 to 2026-07-06):
  - Technology Stack Constraints: toolchain version management (mise), git hook management
    (lefthook), Docker runtime base image decision (Debian slim, not Ubuntu/Alpine), CJK font
    bundling (fonts-noto-cjk) for server-side text rasterization, concurrency model
    (tokio for I/O-bound, rayon for CPU-bound, bridged via spawn_blocking); repository-layout
    declaration (single monorepo: one git repo, one Cargo workspace, one frontend app managed as
    a `pnpm` workspace, with a consolidated top-level tree); Rust build-acceleration tooling
    (sccache, mold, Swatinem/rust-cache — explicitly not Turborepo/Turbopack, which don't
    accelerate rustc or apply to this project's already-fixed Vite bundler); and the project's
    own license (MIT, `LICENSE` at repo root — compatible with Apache-2.0 dependencies already
    chosen, and the concrete reason GPL-3.0 code like Koharu was correctly avoided; all
    2026-07-06)
  - Engineering Workflow & Quality Gates: new "Automated, non-bypassable local quality gates"
    bullet (cargo check/cargo fmt/eslint enforced via lefthook + CI); new "Dependencies default to
    the latest stable release, verified at implementation time" bullet (2026-07-06 — prompted by
    checking `ort`'s version for Phase 2 and discovering both that the pinned version was already
    current and that a more complete crate, `oar-ocr`, now exists on top of it)
  - Principle III: new bullet mandating Phase 1 concurrency planning + a benchmark suite
    comparing bulk-operation throughput against the previous system (mirrors the new
    concurrency-benchmarking user story added to specs/001-lanrurugi-full-rewrite/spec.md)
Removed sections: none
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ reviewed — "Constitution Check" gate is generic
    and reads from this file at plan time; no edit needed.
  - .specify/templates/spec-template.md ✅ reviewed — no constitution-specific references;
    no edit needed. (Tooling/reuse-source details like the ESLint/lefthook template below are
    implementation detail and deliberately kept out of spec.md per its own quality checklist.)
  - .specify/templates/tasks-template.md ✅ reviewed — generic task/user-story structure. The
    Setup phase of the next `/speckit-tasks` run for specs/001-lanrurugi-full-rewrite MUST include
    a concrete task to adapt ESLint/lefthook/mise config from the user's existing
    `~/jellyfin-suite` project (`apps/frontend/eslint.config.mjs`, `lefthook.yml`, `.mise.toml` —
    same Rust + React19/TS/Vite stack) rather than authoring rules from scratch. Tracked in memory
    (`jellyfin-suite-tooling-reference`) as a durable pointer until that tasks.md exists.
  - .specify/templates/checklist-template.md ✅ reviewed — generic; no edit needed.
  - Command files under .specify/templates/commands/ — none present in this repo layout
    (commands are Claude skills under .claude/skills/); no edit needed.
Follow-up TODOs:
  - None. Prior TODO(RATIFICATION_DATE) from v1.0.0 was already resolved.
-->

# LANrurugi Constitution

## Core Principles

### I. Legacy Data & User-Trust Compatibility (NON-NEGOTIABLE)

LANrurugi is a from-scratch Rust + React reimplementation of LANraragi, but it MUST remain a
drop-in continuation of a user's existing library, not a fresh start. Concretely:

- Existing Redis-stored data (archive records, tags, categories, reading progress, thumbnails,
  tankoubon groupings, configuration) produced by legacy LANraragi MUST remain readable by
  LANrurugi without requiring a destructive migration as a precondition for basic operation.
- Any change to a core identity or integrity algorithm inherited from LANraragi (e.g. archive ID
  computation) MUST preserve read-compatibility with data already produced by the legacy
  algorithm. A new/improved algorithm may become the default for newly scanned content, but it
  MUST ship alongside an explicit, user-triggered migration ("rebuild index") path before the
  legacy algorithm is ever removed — it MUST NOT be silently swapped in place.
- Known behavioral quirks of the legacy implementation that are being deliberately fixed (e.g.
  the SHA-1-of-first-512KB archive ID causing false duplicate-merging of distinct archives that
  share a leading byte range) MUST be fixed going forward without breaking reads of data already
  keyed under the old behavior. Fixes are additive/opt-in upgrades, not silent rewrites of
  existing identities.
- Rationale: the target users already have years of tagging, favorites, and reading-progress
  history in Redis. Silently invalidating that data is the single largest adoption risk for this
  project and is treated as a correctness bug, not a migration inconvenience.

### II. API Contract Fidelity (Phase 1)

The Rust backend MUST honor the existing LANraragi REST API contract (as documented in
`tools/openapi.yaml` in the legacy repository) for the duration of Phase 1: same endpoints,
request/response shapes, and API-key authentication semantics. Third-party clients that already
integrate with LANraragi (e.g. Tachiyomi/Mihon extensions, community scripts, OPDS readers) MUST
continue to work against LANrurugi without modification.

- Deliberate breaking changes to request/response shape MUST be introduced as a new, explicitly
  versioned API surface (e.g. `/api/v2/...`) rather than silently altering the existing `/api`
  paths relied upon by the ecosystem.
- New, LANrurugi-only functionality MAY freely add new endpoints; this principle constrains
  changes to *existing* endpoints only.

### III. Resource-Conscious, Genuinely Concurrent Single-Process Architecture

A primary motivation for this rewrite is lower idle resource usage and a simpler deployment
footprint than the legacy multi-process Perl stack (web process + Minion workers + Shinobu file
watcher, all coordinating through Redis) — and, deliberately, *real* multi-core concurrency where
the legacy Perl implementation had none, not just a byproduct of switching language.

- The historically separate Shinobu (file watcher) and Minion (background job queue) processes
  MUST be consolidated into asynchronous tasks running inside the main Tokio runtime of the single
  LANrurugi binary, rather than reintroduced as separate long-running processes.
- Component/engine choices MUST weigh runtime memory and startup footprint as a first-class
  criterion alongside functionality — e.g., preferring subprocess isolation for the plugin runtime
  over embedding a full browser-grade JS engine (V8) directly inside the main server process.
- Performance-sensitive paths (archive scanning, hashing, thumbnail generation) SHOULD prefer
  algorithms/libraries with real-world throughput advantages (e.g. BLAKE3 over SHA-1/256 for new
  hashing work) but MUST NOT be chosen at the expense of Principle I (data compatibility).
- **Concurrency is a first-class design goal, planned in Phase 1, not deferred.** Weak
  parallelism was a named, concrete weakness of the legacy implementation (Perl's interpreter-level
  threading limitations meant bulk work — library scans, hashing, thumbnailing — ran effectively
  single-threaded, with Minion/Shinobu using separate OS processes rather than shared-memory
  parallelism). Bulk, CPU-bound operations (archive-identity hashing during scans and the
  duplicate-repair reindex, thumbnail/image decode-and-resize) MUST be parallelized across
  available cores rather than processed sequentially. I/O-bound operations (request handling,
  Redis access, plugin subprocess execution, file watching) MUST be handled asynchronously so many
  operations proceed concurrently without one-thread-per-operation overhead. Phase 1 planning MUST
  include a benchmark suite comparing bulk-operation throughput against the previous system on the
  same hardware (see the feature spec's concurrency-benchmarking user story), so this improvement
  is measured, not assumed.

### IV. Sandboxed, Language-Agnostic Plugin Extensibility

LANrurugi replaces LANraragi's dynamically-loaded Perl plugin system with JavaScript/TypeScript
plugins (metadata scrapers, login handlers, downloaders) executed via the Deno CLI runtime.

- Plugins MUST run as an isolated subprocess (or a persistent worker-pool of subprocesses),
  never embedded in-process in the main server binary, so a misbehaving or malicious plugin cannot
  destabilize or gain unrestricted access to the host process.
- Plugin subprocesses MUST run under Deno's permission model (explicit `--allow-net` /
  `--allow-read` / etc. grants) scoped to what each plugin actually declares it needs, rather than
  broad/unrestricted permissions by default.
- To bound latency, plugin execution SHOULD use a persistent worker pool with dynamic `import()`
  dispatch rather than spawning a fresh Deno process per invocation.
- TypeScript plugins run natively under Deno; no separate transpile step is required as a
  precondition for a plugin to execute.

### V. Secrets & Network Trust Boundaries

Phase 2 introduces user-selectable LLM backends (OpenAI-compatible, Anthropic, and local Ollama)
for on-page translation. These have fundamentally different trust boundaries and MUST be treated
accordingly:

- Cloud provider API keys (OpenAI-compatible, Anthropic) MUST be stored and used server-side only.
  They MUST NOT be transmitted to, stored in, or made readable by browser-side code (no
  `localStorage`/client JS holding a raw provider key). Requests to cloud providers MUST be
  proxied through the Rust backend.
- Local-model traffic (browser → user's own loopback Ollama instance) is architecturally
  different: the server cannot reach the browser's local device, so this path necessarily
  originates from the browser. It MUST be implemented via standards-compliant, opt-in mechanisms
  (correct CORS + Private Network Access response headers on the local endpoint, or an explicit
  local companion process/extension that the user consciously installs) — never by instructing
  users to disable browser security features (e.g. launching a browser with web security
  disabled) as a supported workaround.
- Any feature that depends on browser-to-loopback connectivity MUST degrade gracefully (detect
  failure, explain why, offer a fallback) rather than assuming it always succeeds, since browser
  vendor policy in this area (Private Network Access) evolves independently of this project.

### VI. Phased Scope Discipline

This project is explicitly split into two phases, and specs/plans MUST respect that boundary:

- **Phase 1** delivers feature parity with the existing LANraragi archive-management, reader,
  plugin, and API surface, reimplemented in Rust (Tokio/Axum) + React, with Redis as the
  compatible data store.
- **Phase 2** (OCR text-region detection/merging, volume-level font-lock caching, client-selectable
  LLM translation backends) is a distinct, later body of work. Phase 2 concerns MUST NOT be
  smuggled into Phase 1 specs/plans/tasks, and MUST NOT block or gate Phase 1 delivery.
- Phase 2 features, once built, MUST be optional/toggleable such that a user who never enables
  them sees no behavioral or performance regression in the Phase 1 experience.

## Technology Stack Constraints

- **Repository layout**: A single monorepo — one git repository, one Cargo workspace, one
  frontend app — not multiple repositories and not multiple independent frontend packages.
  Every phase (Phase 1, Phase 2, and any future phase) adds crates to the same workspace and
  modules to the same `frontend/` app rather than creating a new one; this is a hard constraint,
  not a per-feature-plan choice (see constitution Principle VI for how Phase 2's plan.md
  documents this: "adds to the existing Phase 1 workspace/app rather than creating a new one").
  Top-level layout:
  ```text
  Cargo.toml, Cargo.lock       # workspace root
  pnpm-workspace.yaml, pnpm-lock.yaml  # frontend package management (pnpm)
  .mise.toml                    # pinned toolchain versions
  lefthook.yml                  # git hooks (cargo check/fmt, eslint)
  Dockerfile                    # Debian-slim, multi-stage (Deno binary, CJK fonts, model assets)
  .github/workflows/            # CI (authoritative quality gate)
  crates/                       # Cargo workspace members — see each phase's plan.md for the
                                 # current member list; Phase 1 defines the base set, Phase 2
                                 # (and later phases) add to it, never fork it
  frontend/                     # single React 19 + TS + Vite app (a pnpm workspace package);
                                 # each phase adds src/ subdirectories/components, never a
                                 # second app — pnpm's workspace support is there for efficient,
                                 # deduplicated installs and headroom to add a shared package
                                 # (e.g. common UI) later, not to fragment the frontend today
  bench/                        # cross-system/microbenchmark harness (Phase 1 US8)
  scripts/                      # standalone operational scripts (e.g. model acquisition)
  docs/                         # supplementary docs referenced by tasks (e.g. per-feature
                                 # setup guides) — not a replacement for spec-kit's specs/
  specs/                        # spec-kit planning artifacts (not shipped code)
  ```
- **Project license**: MIT (`LICENSE` at repo root), for the LANrurugi project itself. This is
  independent of, and compatible with, the licenses of dependencies chosen elsewhere in this
  document — Apache-2.0 dependencies (e.g. `oar-ocr`, kha-white's `manga-ocr`) are freely usable
  from an MIT project with no copyleft interaction; GPL-3.0 code (e.g. Koharu, deliberately
  avoided as a dependency per `specs/002-ocr-manga-translation/research.md` §1) would not be,
  which is one more concrete reason that avoidance decision was correct, not just cautious.
- **Backend**: Rust, Tokio async runtime, Axum web framework.
- **Concurrency model**: `tokio` for all I/O-bound async work (HTTP handling, Redis access,
  plugin subprocess execution, file watching). `rayon` for CPU-bound data-parallel work (bulk
  archive-identity hashing during scans/reindex, thumbnail/image decode-and-resize). Rayon (or any
  other blocking CPU-bound work) MUST be dispatched via `tokio::task::spawn_blocking` (or an
  equivalent bridge), never run directly on a Tokio worker thread, so bulk CPU-bound work cannot
  stall concurrent request handling. See Principle III.
- **Data store**: Redis (via an async Rust Redis client), reused as-is from the legacy deployment
  model — see Principle I. Introducing an additional relational/embedded store requires an
  amendment to this constitution, not an ad-hoc decision in a feature plan.
- **Frontend**: React 19 + TypeScript, built with Vite, styled with Tailwind CSS, shipped as a
  static SPA served by the Axum backend. State/data layer: Zustand for client state, TanStack
  Query for server-state synchronization and caching. Package manager: `pnpm`, using its
  workspace support (see Repository layout above) even though there is currently one frontend
  package — not `npm`/`yarn`, and not a JS/TS task-orchestrator layer like Turborepo on top of it
  (see Rust build acceleration below for why that class of tool doesn't help here either).
- **Plugin runtime**: Deno CLI, invoked as a subprocess per Principle IV. The Deno binary is
  obtained via a pinned-version, multi-stage Docker build (e.g. copying from an official
  minimal Deno image, or a checksum-verified release download) — never an unpinned `latest` or an
  unverified `curl | sh` install in the production image.
- **Rust build acceleration**: `sccache` (`RUSTC_WRAPPER=sccache`) for shared compilation-object
  caching across local rebuilds and CI, and `mold` as the local/CI linker on Linux (a Cargo
  workspace with this many member crates is linker-bound as much as compiler-bound). CI
  additionally uses the `Swatinem/rust-cache` GitHub Action to cache `~/.cargo` and `target/`
  between runs. JS/TS-monorepo task orchestrators (e.g. Turborepo) are explicitly out of scope for
  this purpose — they cache task invocations by input hash, which is strictly worse than Cargo's
  own incremental-compilation understanding of the crate dependency graph, and don't accelerate
  `rustc` itself. Bundler-level tools (e.g. Turbopack) are unrelated: they address JS/TS frontend
  bundling speed, not Rust compilation, and this project's frontend bundler choice (Vite) is
  already fixed above.
- **Archive identity algorithm**: new scans use a "size-aware" fingerprint —
  `hash(first 512KB of file bytes ++ u64 big-endian file size)` — as the default going forward.
  Legacy `SHA-1(first 512KB)` remains supported as a read-compatible fallback for data migrated
  from LANraragi, per Principle I. The specific hash function (e.g. BLAKE3) is an implementation
  detail to be finalized in Phase 1 planning, but the byte-layout (content sample, then size,
  fixed-width big-endian) is fixed by this constitution.
- **LLM provider abstraction (Phase 2)**: internal translation-request/response types are modeled
  on the OpenAI Chat Completions shape as the common denominator (also covers Ollama via its
  OpenAI-compatible `/v1` endpoint); a distinct adapter handles Anthropic's native Messages API
  format (separate `system` field, content-block array, `x-api-key`/`anthropic-version` auth,
  mandatory `max_tokens`, distinct SSE streaming event shape).
- **Toolchain version management**: `mise`, with a manifest (e.g. `.mise.toml`) checked into the
  repository pinning the exact Rust, Node, and Deno versions used for local development, CI, and
  Docker builds. Contributors and CI MUST NOT rely on whatever toolchain versions happen to be
  preinstalled on a given machine.
- **Git hook management**: `lefthook`, with its configuration checked into the repository, driving
  the pre-commit/pre-push checks defined in Engineering Workflow & Quality Gates below.
- **Docker runtime base image**: Debian slim (`bookworm-slim` or newer), not Ubuntu and not
  Alpine. Alpine is excluded because the official Deno distribution required by Principle IV
  targets glibc, not musl. Debian slim is preferred over Ubuntu because it carries less
  workstation/VM-oriented overhead in a headless service image (Principle III) and matches the
  base library family of the official Deno image used for the plugin-runtime multi-stage build.
- **CJK font bundling**: the runtime image MUST include a CJK-capable font package (e.g.
  `fonts-noto-cjk`, not a Chinese-only package, and not the much larger `-extra` variant) so that
  any server-side rasterization of archive titles/text (e.g. placeholder cover generation, future
  export features) renders Japanese/Chinese/Korean text correctly instead of producing missing-
  glyph boxes. This is unrelated to, and does not substitute for, Phase 2's on-page translation
  rendering, which happens client-side in the browser and depends on the browser's own font stack,
  not the server image's fonts.

## Engineering Workflow & Quality Gates

- **Verify against source, not memory or assumption.** Any design decision that depends on how
  legacy LANraragi actually behaves (algorithms, data layout, edge-case handling, undocumented
  quirks) MUST be confirmed by reading the actual Perl source (or its tests) before being encoded
  into a LANrurugi spec, plan, or implementation. Prior discussion in this project already
  surfaced real discrepancies this way (e.g. the true archive-ID algorithm, and Shinobu's
  silent-merge behavior on ID collision) — assumptions here have repeatedly been wrong in ways
  that would have broken user data or product decisions if uncorrected.
- **Migration tooling is part of the feature, not a follow-up.** Any change that alters the shape
  or keying of persisted data (per Principle I) MUST ship its migration/rebuild tooling in the
  same feature, not as a deferred task.
- **Compatibility-affecting changes require explicit sign-off.** Changes touching the API surface
  (Principle II) or on-disk/Redis data compatibility (Principle I) MUST be called out explicitly
  in the relevant spec's Assumptions/Requirements sections, not left implicit.
- **Cost- and rate-limit-aware defaults.** Features that can trigger automatic background calls to
  metered external services (e.g. sliding-window pre-fetch against a paid LLM API in Phase 2)
  MUST default to conservative behavior for metered providers and MAY be more aggressive by
  default only for zero-marginal-cost local backends.
- **Automated, non-bypassable local quality gates.** Every commit MUST pass, at minimum, a fast
  Rust compile check (`cargo check`) and format check (`cargo fmt --check`) for backend changes,
  and lint (`eslint`) and the project's formatter for frontend changes. These checks MUST run
  automatically via the `lefthook`-managed pre-commit/pre-push hooks (see Technology Stack
  Constraints) so they do not depend on individual contributor discipline, and MUST also run in CI
  as the authoritative gate (the local hook is a fast-feedback convenience, not a substitute for
  CI). Slower checks (full test suite, `cargo clippy`) SHOULD run at pre-push or in CI rather than
  on every commit, to keep the commit-time gate fast enough to not be worth bypassing.
- **Dependencies default to the latest stable release, verified at implementation time — not a
  remembered or copied-over version.** When a Setup task adds a new crate/npm dependency, it MUST
  pin whatever is actually the latest stable release on crates.io/npm at that moment (checked
  live), not a version merely cited in an earlier research.md/plan.md or copied from a sibling
  project's Cargo.toml. A version number appearing in a design document is a starting candidate to
  reverify, never a hard pin to match exactly — if a newer stable release exists by
  implementation time, prefer it unless there's a documented, specific compatibility reason not
  to (e.g. a known regression, an unsupported MSRV). This applies equally to discovering whether a
  more complete crate now exists that could replace planned custom work (as happened when
  checking `ort`'s version surfaced `oar-ocr`, an existing OCR crate built on it).

## Governance

This constitution supersedes ad-hoc technical preferences for anything within its scope. Where a
feature spec or plan conflicts with a principle here, the spec/plan MUST either be revised to
comply or the conflict MUST be resolved by amending this constitution first — a plan MUST NOT
silently override a principle.

**Amendment procedure**: Amendments are proposed via the same `/speckit-constitution` workflow,
must update the version per the policy below, and must re-run the consistency propagation check
against `.specify/templates/*` before being considered complete.

**Versioning policy** (semantic versioning applied to governance content):
- **MAJOR**: Backward-incompatible removal or redefinition of a principle (e.g. dropping the
  Redis-compatibility guarantee, or abandoning Phase 1/Phase 2 scope separation).
- **MINOR**: A new principle or materially expanded constraint is added (e.g. adding a new
  Phase 2 sub-principle once that phase's design solidifies).
- **PATCH**: Clarifications, wording fixes, or non-semantic refinements that don't change what is
  required or forbidden.

**Compliance review**: Every `/speckit-plan` run MUST re-check its Constitution Check gate against
the current version of this file; every `/speckit-specify` output touching data compatibility, the
API surface, or Phase 1/Phase 2 boundaries MUST be reviewed against Principles I, II, and VI before
being marked ready for planning.

**Version**: 1.6.0 | **Ratified**: 2026-07-05 | **Last Amended**: 2026-07-06
