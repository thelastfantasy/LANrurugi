# Implementation Plan: AI Plugin Creation Wizard

**Branch**: `006-ai-plugin-wizard` | **Date**: 2026-08-24 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `/specs/006-ai-plugin-wizard/spec.md`

## Summary

A wizard that generates login/metadata/download plugins from a natural-language description of a
target site, backed by DeepSeek's tool-calling API so the model can request the system fetch a real
page (or a supplementary reference URL) mid-generation rather than guessing blind from text alone.
Generated drafts are trial-run through the existing Deno plugin sandbox (a temporary, per-session
staging file under `plugins/custom/_wizard/`, reusing `PluginPool::plugin_info`/`execute` exactly as
`upload_plugin` already does, just without the "move to a permanent category directory on success"
step until the user explicitly confirms). All session/draft-history state lives in the frontend
(matching the spec's own assumption that history only needs to survive the session, not a refresh);
every backend endpoint the wizard calls is stateless — one call in, one result out, no server-held
"wizard session" object.

## Technical Context

**Language/Version**: Rust (stable channel, pinned via `mise`, matching 001) for the new wizard API
endpoints, LLM tool-calling extension, and page-fetch/redirect-tracking logic; TypeScript (React 19)
for the new wizard frontend pages and CodeMirror 6 integration. No new Deno-side code — draft
plugins are plain `.ts` files executed by the *existing* dispatcher/plugin-sdk contract, unchanged.

**Primary Dependencies**:
- Backend: `reqwest` (already a workspace dependency, `stream` feature already enabled per 005) for
  the wizard's own page-fetch tool (target pages, auxiliary reference URLs), using a custom
  `redirect::Policy` to both cap the hop count and record the full trail (see research.md); no new
  crate needed for HTTP. `lanrurugi-llm` gains a new function alongside the existing `chat`/
  `json_chat` that accepts a full `messages` history plus a `tools` array and returns either a final
  text/JSON answer or a list of `tool_calls` to execute (DeepSeek's tool-calling wire format is
  OpenAI-compatible — see research.md) — no new dependency, same `reqwest` HTTP client the crate
  already uses. Draft trial-running reuses `lanrurugi-plugin::PluginPool` as-is (no crate change) by
  writing the draft to a temporary staging path first, mirroring `crates/lanrurugi-api/src/
  plugins.rs::upload_plugin`'s existing stage → validate → (this feature: run, then discard instead
  of promote) pattern.
- Frontend: `@uiw/react-codemirror` (React wrapper) + `@codemirror/lang-javascript` (TypeScript
  highlighting via `javascript({ typescript: true })`) + `codemirror` (bundled basic setup) — this
  project's first code-editor dependency (verified latest stable versions in research.md, per
  constitution's "verify at implementation time" dependency rule). No other new frontend dependency
  — history/session state is plain React state (`useState`/`useReducer`), no new state-management
  library needed for a single-page wizard flow.

**Storage**: Redis (reused as-is, per constitution Principle I) — none, for this feature specifically.
Wizard session/draft-revision/trial-run-result state is intentionally *not* persisted anywhere
(frontend-only, per the spec's own Assumptions — "history only needs to survive the session"); the
only persisted artifact is the final `.ts` file once the user confirm-saves, written straight to the
existing `plugins/<category>/` layout `discover_namespaces` already scans — no new Redis key
namespace introduced.

**Testing**: `cargo test` (unit tests for the LLM tool-calling message-loop plumbing against a
mocked HTTP responder, the redirect-trail-capturing fetch helper, and the stage/trial-run/discard
draft-execution path); Deno's own `deno check` is not applicable here (drafts are user/AI-authored
arbitrary `.ts`, not shipped plugin source under version control). A Playwright E2E layer (003's
established pattern) for the wizard's own multi-step UI journey was attempted but removed
(2026-08-27, explicit user decision) after surfacing real bugs elsewhere (a `PUT /settings`
allowlist rejecting `llm_api_key` outright, a mock-LLM-server port collision) without reaching a
stable pass — the manual verification steps in `quickstart.md` are this feature's test coverage for
now.

**Target Platform**: Linux server (unchanged from 001 — adds to the existing single binary).

**Project Type**: Web application (unchanged from 001 — adds to the existing Rust backend + React
SPA, not a new deployable).

**Performance Goals**: SC-004 (every trial-run result, including each link in a multi-link scenario,
surfaces within a reasonable wait with no "stuck, no feedback" state) — informs a per-tool-call and
per-trial-run timeout in the implementation, not a specific millisecond target (LLM response latency
is an external, non-controllable variable, so no fixed number is fixed by this plan; see research.md
for the chosen timeout values and their rationale).

**Constraints**: Constitution Principle I (draft trial-running and saving must never corrupt or
overwrite an existing, real installed plugin — enforced by namespace-conflict rejection at save time,
per spec FR-021, and by never writing wizard staging files into a real category directory before
confirm-save); Principle IV (draft trial-running reuses the *exact* same sandboxed subprocess
permission model real plugins get — no relaxed/expanded permission surface for "it's just a draft");
Principle V is Phase 2-scoped (LLM secret handling) but its spirit is honored here for the same
reason: the DeepSeek API key stays server-side only (reusing the existing `resolve_api_key`
Redis-config/env-var mechanism), never exposed to the browser, and per spec FR-012, login test
credentials specifically are additionally never sent to the LLM at all, a stricter bar than Principle
V's cloud-provider-key handling since credentials here are the *user's own third-party site*
credentials, not this project's own secret.

**Scale/Scope**: 7 user stories (P1×4, P2×3), 25 functional requirements (FR-001–FR-025), 4 key
entities (all frontend-only, per Storage above), single-owner/single-instance deployment scope
matching 001 — no multi-tenant concerns.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Gate | Status |
|---|---|---|
| I. Legacy Data & User-Trust Compatibility | No existing Redis key shape changes; draft handling must never corrupt/overwrite a real installed plugin | **PASS** — this feature introduces no new persisted data at all (session state is frontend-only); the only write to persistent storage is the final confirm-save, which goes through the same namespace-conflict-rejection path `upload_plugin` already uses (FR-021) |
| II. API Contract Fidelity (Phase 1) | Existing `/api/*` endpoints must keep working unmodified | **PASS** — this feature only adds new, additive endpoints under a new `/api/plugin-wizard/*` prefix; no existing endpoint's request/response shape changes |
| III. Resource-Conscious, Genuinely Concurrent Architecture | New I/O-bound work (LLM calls, page fetches, trial-run subprocess calls) must be genuinely async, not blocking | **PASS** — all new work is `tokio`-async: `reqwest` for both the LLM HTTP calls and the page-fetch tool, `PluginPool`'s existing async `execute`/`plugin_info` for trial runs; no new CPU-bound bulk work is introduced (this is single-item, per-request work, not a batch job), so Principle III's `rayon`-parallelization bullet doesn't apply here |
| IV. Sandboxed, Language-Agnostic Plugin Extensibility | Plugin sandbox model / permission grants must not be weakened for drafts | **PASS** — draft trial-running reuses `PluginPool`'s existing two-phase permission model (zero-permission `plugin_info()` probe, then a real run scoped to whatever the draft itself declares) completely unmodified; a draft gets no more trust than a real, saved plugin ever would |
| V. Secrets & Network Trust Boundaries | N/A (Phase 2-only) in letter, but its spirit is directly relevant here | **PASS (by analogy)** — the DeepSeek API key is server-side only via the existing `resolve_api_key` mechanism, never reaches the browser (matches Principle V's cloud-key handling exactly); additionally, per spec FR-012 (stricter than Principle V requires), login test credentials are never sent to the LLM at all — the local system executes the real login call and only ever hands AI a sanitized outcome |
| VI. Phased Scope Discipline | Must not smuggle Phase 2 concerns in, must not block Phase 1 | **PASS** — this is a Phase 1 addendum (like 002/003/005), independent of Phase 2's OCR/translation work; does not block or get blocked by 004 |
| VII. Frontend Engineering Discipline & Legacy UI Fidelity | New pages/components must follow the one-component-per-`index.tsx`/shared-file rule; no legacy UI is being reproduced here (this is wholly new functionality with no legacy equivalent), so the legacy-fidelity bullets don't apply | **PASS** — the wizard is new frontend surface with no legacy reference to verify against; page-file organization is enforced during implementation per the existing convention (see Project Structure below for the planned file layout) |

No violations requiring Complexity Tracking.

## Project Structure

### Documentation (this feature)

```text
specs/006-ai-plugin-wizard/
├── plan.md              # This file (/speckit-plan command output)
├── research.md          # Phase 0 output (/speckit-plan command)
├── data-model.md        # Phase 1 output (/speckit-plan command)
├── quickstart.md        # Phase 1 output (/speckit-plan command)
├── contracts/           # Phase 1 output (/speckit-plan command)
└── tasks.md             # Phase 2 output (/speckit-tasks command - NOT created by /speckit-plan)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-llm/
│   └── src/
│       └── lib.rs                # NEW: tool_chat() — accepts a full `messages` history +
│                                  # a `tools` array, returns either final content or
│                                  # `tool_calls` to execute; existing chat()/json_chat() untouched
├── lanrurugi-api/
│   ├── policy/
│   │   └── route_policy.csv      # +12 rows: deny token_admin/token_guest/anonymous on each of
│   │                              # the 4 new POST endpoints below (FR-023, Session-only) —
│   │                              # same Casbin mechanism api_tokens.rs's own routes already use,
│   │                              # not a new middleware
│   └── src/
│       └── plugin_wizard/        # NEW module
│           ├── mod.rs            # router: POST /plugin-wizard/lookup, /generate,
│           │                     # /trial-run, /save (all stateless, per Technical Context)
│           ├── lookup.rs         # domain → installed-plugin coverage (reuses
│           │                     # discover_namespaces + plugin_info(), same as plugins.rs)
│           ├── generate.rs       # the tool-calling agentic loop: system prompt (SDK types +
│           │                     # same-domain sample code, FR-009) + user prompt, dispatches
│           │                     # the fetch_page tool call locally, loops until AI returns
│           │                     # final code
│           ├── fetch.rs          # NEW: reqwest GET with a custom redirect::Policy that both
│           │                     # caps hop count (FR-011) and records the full trail;
│           │                     # shared by generate.rs's fetch_page tool and by trial-run's
│           │                     # own target-link fetches (drafts call this indirectly by
│           │                     # actually executing, not by AI requesting it — see research.md)
│           └── trial_run.rs      # stage draft .ts to plugins/custom/_wizard/<uuid>.ts,
│                                 # call PluginPool::plugin_info + execute exactly as
│                                 # upload_plugin does, always delete the staging file
│                                 # afterward (never promoted to a real category dir here —
│                                 # only /save does that, reusing upload_plugin's own
│                                 # validate-then-move logic)

plugins/
└── custom/
    └── _wizard/                  # NEW, gitignored-equivalent runtime-only staging dir —
                                   # never scanned as real plugins (discover_namespaces already
                                   # only reports what list_plugins/execute callers ask for by
                                   # namespace; this directory is simply never referenced by
                                   # the normal plugin-listing UI, see research.md)

apps/frontend/
└── src/
    └── pages/
        └── PluginWizard/          # NEW page directory
            ├── index.tsx          # default-exported page component only (Principle VII)
            ├── DomainLookupStep.tsx
            ├── TypeSelectionStep.tsx
            ├── GenerationStep.tsx
            ├── CodeEditor.tsx     # thin wrapper around @uiw/react-codemirror
            ├── TrialRunResult.tsx
            ├── DraftHistoryPanel.tsx
            └── useWizardSession.ts  # the in-memory session/draft-history state machine
                                      # (per Storage above — no backend persistence)
```

**Structure Decision**: Adds to the existing Phase 1 Cargo workspace/frontend app per constitution
Technology Stack Constraints (no new crate — `plugin_wizard` is a new module inside the existing
`lanrurugi-api` crate, since it's tightly coupled to that crate's existing `AppState`/`PluginPool`/
auth-context wiring, the same reasoning 005's download-manager module used). No new Deno-side code.
One new frontend page directory following Principle VII's file-organization rule.

## Complexity Tracking

*No Constitution Check violations — table omitted.*
