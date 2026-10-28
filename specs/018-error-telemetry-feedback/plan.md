# Implementation Plan: Error Telemetry and Feedback Bundles

**Branch**: `018-error-telemetry-feedback` | **Date**: 2026-10-28 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/018-error-telemetry-feedback/spec.md`

## Summary

Replace the legacy-shaped `/logs` page with a diagnostics surface that works on an isolated network:
both sides capture errors automatically into a bounded local store, an administrator reads them as
grouped issues and builds a scrubbed feedback bundle plus a bilingual issue text, and — only when the
administrator configures a self-hosted Sentry-protocol tracker (Rustrak is the reference) and opens a
time-boxed window — the same captured envelopes are also forwarded there.

The load-bearing decision: **the instance is its own Sentry-compatible ingest**. The browser SDK posts
the standard envelope to this instance's own `/api/diagnostics/envelope` (same origin, session
cookie), so capture needs no third-party service and no custom client transport; the server SDK's
transport writes into the same store. Forwarding is then a second sink over the stored envelope, never a
second capture path.

## Technical Context

**Language/Version**: Rust (workspace toolchain from `rust-toolchain.toml`, currently 1.99) on the
server; TypeScript + React 19 on the client, built by Vite.

**Primary Dependencies**: `sentry` 0.49.x (MIT) with `sentry-tower` (axum integration),
`sentry-tracing` (error-level events) and `sentry-panic`; `@sentry/react` 11.x for the client;
existing workspace crates `zip` (streamed bundle writer, already used by `tankoubon_zip.rs`),
`governor` (rate limiting, already used by `share_attempts.rs`), `serde_json`, `tokio`, and the Redis
repository layer (`deadpool-redis`). New client dependency for optional local symbolication:
`@jridgewell/trace-mapping` (small, MIT). All versions are re-verified live at implementation time per
the constitution's dependency rule.

**Storage**: Redis, additive keys only (`LANRURUGI_DIAGNOSTIC_EVENT:<id>`,
`LANRURUGI_DIAGNOSTICS_EVENTS` sorted set, `LANRURUGI_DIAGNOSTIC_ISSUE:<fingerprint>`,
`LANRURUGI_DIAGNOSTICS_ISSUES`, `LANRURUGI_DIAGNOSTICS_DROP:*` counters, and new fields on the existing
`LRR_CONFIG` hash). No existing key changes shape, so no migration tooling is required (Principle I).
Bundle archives are generated on the fly and streamed, never persisted.

**Testing**: `cargo test` per crate (`lanrurugi-api`, `lanrurugi-storage`, `lanrurugi-server`) through
the guarded `mise run test-crate`; `vitest` for the client (fingerprint/selection/text formatting, SDK
init gating, window countdown); Playwright for the settings → diagnostics flow; `cargo clippy`/
`fmt-check` and `eslint`/`tsc` as the standard gates.

**Target Platform**: the existing single Linux server binary (Docker image and native packages) plus
evergreen browsers; the feature must behave identically with no outbound network access.

**Project Type**: web service + SPA — one repository, one binary, one frontend bundle.

**Performance Goals**: capture adds ≤ 5 ms p95 to a request and ≤ 0.6 MB of process memory at rest;
storage stays within the configured limits (defaults 7 days / 50,000 events / 512 MB); a one-hour
bundle against a full store is generated in ≤ 30 s and capped at 20 MB; forwarding adds no latency to
the request that produced the event (background flush).

**Constraints**: offline-capable by default; zero transmission without an explicitly configured
endpoint *and* an open window; every event scrubbed before storage; the whole feature behind a single
runtime switch; no new long-running process (Principle III).

**Scale/Scope**: single-administrator self-hosted instances; up to 50,000 retained events and a few
thousand issues; one new settings section with a list, a detail view, a bundle builder and a forwarding
panel; no change to existing API contracts (new routes only, under `/api/diagnostics`).

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Assessment |
|---|---|
| I. Legacy data & user-trust compatibility | Pass. Additive Redis keys only; the legacy `/logs/{category}` route and its files are untouched; no archive, share or queue record changes shape. No migration tooling needed. |
| II. API contract fidelity | Pass, and explicitly bounded: the feature adds `/api/diagnostics/*` routes and settings entries and changes no existing response or parameter. New routes are documented in `docs/api/sidecar/diagnostics.yaml`. |
| III. Resource-conscious single process | Pass. No new service and no new long-running task beyond one flush task; bounded store with enforced eviction; the request path only accepts and hands off an envelope; bundle generation is one batched blocking job (the existing `tankoubon_zip` pattern), not a per-item `spawn_blocking` loop. |
| IV. Sandboxed plugins | Not affected. Plugin failures surface through the existing `tracing` error path, which becomes a capture source; no plugin API change. |
| V. Secrets & network trust boundaries | Pass by design. The tracker address/project key is administrator-only and stored server-side; the browser never receives the key; the only outbound destination is the configured tracker, only while a window is open; scrubbing runs before storage, so neither the store nor an export can contain a share code, token, cookie or path. |
| VI. Phase 1 scope discipline | Pass. Diagnostics is Phase 1 infrastructure (it exists so self-hosters can report defects); nothing from the Phase 2 OCR/translation scope is touched, and the feature can be switched off per instance. |
| VII. Frontend discipline & legacy fidelity | Pass. The settings panel is its own component, the list uses real CSS classes (no inline layout), responsive behaviour is expressed in CSS, and verification is behavioural (curl and a browser walkthrough by the user), not screenshot-guessing. |
| Workflow: dependency & compatibility gates | Pass with the noted version policy (live check at implementation); the only compatibility-affecting surface is *additive* (new routes and settings keys), called out in the spec's Assumptions. |

No violations; Complexity Tracking stays empty.

## Project Structure

### Documentation (this feature)

```text
specs/018-error-telemetry-feedback/
├── plan.md              # this file
├── research.md          # Phase 0 decisions
├── data-model.md        # Phase 1 entities and keys
├── quickstart.md        # Phase 1 validation walkthrough
├── contracts/
│   └── diagnostics-api.md   # administrator routes, envelope ingest, forwarding mapping
├── checklists/
│   └── requirements.md  # spec quality checklist
└── tasks.md             # Phase 2 (/speckit-tasks, not created here)
```

### Source Code (repository root)

```text
crates/lanrurugi-api/src/diagnostics/
├── mod.rs          # init, capture API, runtime switches, the one flush task
├── ingest.rs       # envelope decode, scrubbing, fingerprint, store write
├── store.rs        # Redis repository calls, retention/eviction, counters
├── issue.rs        # grouping, list/detail projection, resolve/ignore
├── bundle.rs       # selection query, streamed zip, manifest + README entries
├── text.rs         # bilingual feedback text (Chinese + English)
├── forward.rs      # window state machine + sending stored envelopes to a tracker
└── routes.rs       # /api/diagnostics/* administrator routes + the envelope endpoint

crates/lanrurugi-storage/src/diagnostics.rs   # Redis keys, entities, repository
crates/lanrurugi-server/src/app.rs            # mounts the routes, applies the sentry-tower layer
crates/lanrurugi-server/src/main.rs           # SDK/panic/log bridge initialisation
apps/frontend/src/diagnostics/
├── sdk.ts          # Sentry.init against this instance's own origin, gating, breadcrumbs
├── api.ts          # query hooks for issues/events/bundle/window
├── DiagnosticsSection.tsx   # settings panel: switches, usage, endpoint, window
├── IssuesModal.tsx          # list (level/time/source filters, multi-select) + detail
└── BundleModal.tsx          # selection, category toggles, preview, download + text
apps/frontend/src/i18n/locales/{zh,en,ja}.json   # new `diagnostics.*` keys
docs/api/sidecar/diagnostics.yaml               # OpenAPI fragment for the new routes
docs/src/content/docs/{,en/}guide/troubleshooting.md  # user-facing how-to
```

**Structure Decision**: the web-application layout already in place — server modules under
`crates/lanrurugi-api` (with the storage layer split into `lanrurugi-storage` as every other domain
does), wiring in `lanrurugi-server`, UI under `apps/frontend/src/diagnostics`, documentation in
`docs/`. The feature owns a new `diagnostics` module on both sides and touches only three existing
call sites (`app.rs` route mount, `main.rs` init, the settings page entry).

## Phases

### Phase 0 — Research (`research.md`)

Resolve: local-ingest envelope design versus a custom client transport; SDK versions and integrations;
where the window state lives and how it is enforced on both sides; retention/quota eviction in Redis;
the scrubbing rules; the fingerprint algorithm; symbolication by release with maps fetched per
referenced chunk; and how the bilingual text is generated deterministically.

### Phase 1 — Design (`data-model.md`, `contracts/`, `quickstart.md`)

Entities and Redis keys; the administrator API (list/filter/select/detail/resolve/ignore); the envelope
ingest endpoint; bundle generation and download; the forwarding configuration and window state machine;
and the validation walkthrough.

### Phase 2 — Tasks (`/speckit-tasks`, later)

Ordered by the spec's priorities: local capture and store (US1), bundle builder and text (US2),
forwarding window (US3), release identity and symbolication (US4), offline replay (US5), then the
`/logs` demotion and documentation.

## Complexity Tracking

No constitution violations; no entries.
