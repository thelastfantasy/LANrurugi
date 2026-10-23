# Implementation Plan: Subscription Orchestrator

**Branch**: `010-subscription-orchestrator` | **Date**: 2026-10-02 | **Spec**: [spec.md](./spec.md)

**Input**: Feature specification from `specs/010-subscription-orchestrator/spec.md`

## Summary

A standing-rule engine that periodically asks source extensions "what is new that matches this?",
filters the answers against user rules, and hands survivors to the existing download queue. It adds
no site knowledge of its own and no second download path.

Three things make this more than a cron loop, and they shape every decision below:

1. **Discovery does not exist yet.** Today's extension entry points act on one already-known item.
   Asking a site "what is new" is a new capability, and it is the only genuinely new extension
   surface this feature introduces.
2. **A missing sign-in silently shrinks the world.** On real sources, searching signed-out returns a
   smaller result set rather than an error. A check that runs unauthenticated therefore *succeeds*
   while under-reporting — so it must be treated as inconclusive, never as "this is everything".
3. **Unattended spending is the main risk.** Matched works default to waiting for confirmation;
   automatic download is opt-in per subscription.

## Technical Context

**Language/Version**: Rust (2021 edition) for the orchestrator; TypeScript for the extension-side
discovery capability; TypeScript/React for the two UI surfaces.

**Primary Dependencies**: No new ones. Scheduling uses `tokio` timers already in the workspace;
time handling uses `chrono`, already present. Discovery runs through the existing sandboxed
extension runtime, which already ships an HTML/XML parser (so RSS/Atom needs no new library).

**Storage**: Redis, as everywhere else in this project — additive key namespaces only, no change to
any existing key shape.

**Testing**: `cargo test` for orchestrator logic (matching, dedup, scheduling decisions as pure
functions); Vitest for frontend units; existing integration-test patterns for the extension call
path.

**Target Platform**: Same single Linux server process as the rest of the application.

**Project Type**: Additive feature inside the existing workspace — a new module in the API crate,
new storage namespaces, two new UI surfaces, one new extension capability. Not a separate
deployable.

**Performance Goals**: Subscription checks are background work and must stay out of the way of
interactive use (SC-006). Checks are infrequent by nature (hours, not seconds), so throughput is not
the concern; *politeness to sources* is — hence the extension-declared minimum interval (FR-004b).

**Constraints**:
- Must not spend download credit unattended unless the user opted in (FR-007a).
- Must not run overlapping checks for one subscription (FR-012).
- Must survive restart without duplicate downloads or lost reservations (FR-013).
- Background work must respect the project's existing resource-capping posture rather than
  competing with interactive requests.

**Scale/Scope**: Personal-library scale — tens of subscriptions, each yielding tens of candidates
per check. Design for correctness and politeness, not for throughput.

## Constitution Check

*GATE: Must pass before Phase 0 research. Re-check after Phase 1 design.*

| Principle | Assessment |
|---|---|
| **I. Legacy data & user-trust compatibility** | **PASS.** Purely additive: new Redis namespaces, no legacy key touched, no change to how existing archives/categories are stored. Legacy has no subscription concept, so there is no contract to preserve. |
| **II. API contract fidelity (Phase 1)** | **PASS.** All new endpoints are additive and live outside the legacy path set. No existing endpoint changes shape. |
| **III. Resource-conscious, genuinely concurrent single-process architecture** | **PASS, with care.** Checks run as background tasks in the same process, following the existing periodic-task pattern. Two obligations: per-subscription non-overlap (FR-012), and not letting checks contend with interactive work (SC-006). No new process, no new runtime. |
| **IV. Sandboxed, language-agnostic plugin extensibility** | **PASS — and this is why the spec refused plugin-authored HTML.** Discovery is a new capability on the *existing* sandboxed runtime, inheriting its permission model unchanged. Settings are rendered by the application from declared fields; no extension-supplied markup is ever injected into the application's pages. |
| **V. Secrets & network trust boundaries** | **PASS.** No new secret store: sign-in stays per extension namespace exactly as today (FR-002e1). Per-subscription credentials were explicitly rejected to avoid a second secret-handling surface. |
| **VI. Phased scope discipline** | **PASS.** Independent of Phase 2 work; does not block or depend on it. |
| **VII. Frontend discipline & legacy UI fidelity** | **PASS, with one note.** Subscription management is a new settings section following the existing section pattern; the reservation list joins the upload page beside the download queue. Legacy has no equivalent screen, so there is no legacy markup to match — the obligation is internal consistency with this project's own components, not legacy parity. |

**No violations requiring justification.** The two places this feature could have broken a principle
(plugin-authored HTML → IV; per-subscription credentials → V) were closed during clarification and
are recorded in the spec's Assumptions.

### Post-Design Re-check (after Phase 1)

Re-evaluated against the generated design artifacts. **Still passing, no new violations.** Three
points were worth re-examining because the design is where they could have slipped:

- **Principle IV (sandboxing)** — the new discovery capability rides the existing runtime and
  permission model unchanged; it adds a function extensions may export, not a new way for them to
  reach the host. Settings remain application-rendered: `contracts/discovery-capability.md` has
  extensions declare interval guidance and sign-in dependency as *data*, never as markup or code the
  application executes on their behalf.
- **Principle III (single process, resource-conscious)** — `data-model.md` settles the one open
  design question from research in favour of a single scheduler rather than one task per
  subscription. That choice is what makes FR-012's non-overlap a local question instead of
  cross-task coordination, and it avoids multiplying idle tasks.
- **Principle I (additive storage)** — every key namespace in `data-model.md` is new. No legacy key
  is read differently, written differently, or reinterpreted.

One design decision deserves naming because it is load-bearing rather than incidental: the
`inconclusive` cycle outcome. It exists so a signed-out check cannot write Seen Work records. Without
it, works a degraded check never had access to would be marked handled forever — a silent omission
with no error to notice. It is recorded in the data model as a first-class outcome rather than left
as an implementation nicety precisely so it cannot be optimised away later.

## Project Structure

### Documentation (this feature)

```text
specs/010-subscription-orchestrator/
├── spec.md              # Feature specification (46 FR, 8 SC, 16 edge cases)
├── plan.md              # This file
├── research.md          # Phase 0 — decisions behind the design
├── data-model.md        # Phase 1 — entities, state, keys
├── contracts/           # Phase 1 — discovery capability + HTTP surface
│   ├── discovery-capability.md
│   └── subscription-api.md
├── quickstart.md        # Phase 1 — how to prove it works end to end
└── checklists/
    └── requirements.md  # Spec quality checklist (16/16)
```

### Source Code (repository root)

```text
crates/
├── lanrurugi-api/src/
│   └── subscriptions/            # NEW — the orchestrator
│       ├── mod.rs                #   HTTP handlers, router
│       ├── scheduler.rs          #   when to check; catch-up; non-overlap
│       ├── matcher.rs            #   pure rule evaluation (heavily unit-tested)
│       ├── runner.rs             #   one check cycle end to end
│       └── reservations.rs       #   reservation list operations
├── lanrurugi-storage/src/
│   └── subscriptions.rs          # NEW — Redis persistence, additive namespaces
└── lanrurugi-plugin/
    ├── dispatcher/plugin-sdk.ts  # EXTEND — declare the discovery capability
    ├── dispatcher/dispatcher.ts  # EXTEND — dispatch the new capability
    └── src/protocol.rs           # EXTEND — its Rust-side mirror

apps/frontend/src/
├── pages/Settings/
│   └── SubscriptionsSection.tsx  # NEW — manage subscriptions (FR-024)
└── pages/Upload/
    └── ReservationGroup.tsx      # NEW — reservation list (FR-017, FR-025)

plugins/discovery/                # NEW directory — discovery capabilities per source
└── ehentai.ts                    #   first worked example
```

**Structure Decision**: The orchestrator is a new module inside the existing API crate rather than a
new crate. It is thin — scheduling, rule evaluation, and delegation — and it leans on the API
crate's existing download-queue, plugin-pool, and activity facilities. A separate crate would mean
either circular dependencies or hoisting those facilities out, neither of which this feature's size
justifies. `matcher.rs` is deliberately split out as pure logic so rule evaluation can be tested
without Redis, a plugin runtime, or a clock.

## Complexity Tracking

No constitution violations to justify. One deliberate complexity worth naming:

| Choice | Why | Simpler alternative rejected because |
|---|---|---|
| Discovery accepts both extension-supplied listings and user-supplied listing URLs through one interface (FR-002c) | Both produce the same thing — candidate works | Two separate mechanisms would double the surface the authoring assistant must generate and the UI must explain, for no difference in outcome |
| Extension-declared minimum interval enforced as a hard floor (FR-004b) | The risk guarded against — getting the user rate-limited or banned — is not the user's to discover by trial | Letting the user set any interval puts a site-safety decision in the hands of whoever knows least about that site |
