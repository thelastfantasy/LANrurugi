# Specification Quality Checklist: On-Page Manga Translation (Phase 2)

**Purpose**: Validate specification completeness and quality before proceeding to planning
**Created**: 2026-07-06
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders
- [x] All mandatory sections completed

## Requirement Completeness

- [x] No [NEEDS CLARIFICATION] markers remain
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Notes

- No [NEEDS CLARIFICATION] markers were needed — the extensive prior design discussion captured
  in `specs/001-lanrurugi-full-rewrite/phase2-design-notes.md` and this project's constitution
  already resolved the load-bearing decisions (backend categories, secrets/trust-boundary split,
  cost-aware prefetch defaults, non-blocking relationship to Phase 1), so this spec documents them
  as Assumptions/FRs rather than open questions.
- This spec intentionally avoids naming specific providers (OpenAI/Anthropic/Ollama), OCR/font
  models, or any Rust/React implementation detail — those live in this feature's own `plan.md`
  once generated, and in the project constitution's Technology Stack Constraints where they're
  already partially fixed project-wide (LLM provider adapter shape, secrets handling).
- 2026-07-06 `/speckit-clarify` session resolved 4 further ambiguities that surfaced only once
  scrutinized closely: target-language selection (previously entirely unaddressed), the
  translation-cache key composition (page + target language + backend, plus a separate,
  cover-excluding font-vote cache), usage-budget visibility (with a page/archive/day/week
  breakdown), and the not-yet-ready (vs. failed) page-display behavior during look-ahead. All
  four were integrated directly into Functional Requirements, Key Entities, Success Criteria,
  Acceptance Scenarios, and Edge Cases — see spec.md `## Clarifications`. FR count grew from 17 to
  20; no checklist item changed pass/fail state (all were already satisfiable, these clarifications
  closed real gaps rather than fixing failures).
- 2026-07-06 (post-clarify) the user raised two more points directly, folded in as additional
  Clarifications rather than a new formal `/speckit-clarify` pass: (1) locally-hosted backend
  selections must be stored client-side/per-device (not portable across devices) while
  cloud/API-key selections stay server-side/account-wide, with the device-local selection taking
  precedence when present — now FR-003; (2) target-language scope is limited to LTR languages
  this phase, RTL explicitly deferred — folded into FR-004. FR count grew from 20 to 21; no
  checklist item changed state.
- 2026-09-06 `/speckit-clarify` session added FR-008a (per-block color/boldness fidelity,
  previously entirely unaddressed) and SC-003a (its measurable outcome, deliberately given a lower
  80% confidence bar than SC-003's 90% since no external prior art validates this heuristic's
  accuracy — see research.md §13). FR count grew from 21 to 22 (with the "a" suffix, not a
  renumber); no checklist item changed pass/fail state — the new requirement and success criterion
  are both testable, measurable, and free of implementation leakage on their own terms.
- 2026-09-06 (second `/speckit-clarify` pass, same day) added FR-007a–e (Terminology Glossary +
  advisory context assembly for name/term and tone/style consistency across independent per-block
  translation requests — previously entirely unaddressed) and SC-003b. Notably, the initial answer
  to the name-consistency question (exact-substring glossary matching alone) was revised
  mid-session after the user raised concrete counter-examples (nickname variants with no substring
  relationship, e.g. さゆき/さっちゃん; initialism abbreviation of a full name) that plain string
  matching cannot catch — the final design is a hybrid (exact match + LLM-recognized variants via
  context), documented as such in research.md §14 rather than silently replacing the earlier
  answer. FR count grew from 22 (with the FR-008a suffix) to 27 (FR-007a through FR-007e); no
  checklist item changed pass/fail state — each new FR and SC-003b remain testable, measurable,
  and implementation-detail-free (they specify *that* context is assembled and *what* it contains,
  not how the assembly or matching is coded).
- 2026-09-06 (third `/speckit-clarify` pass, same day) added FR-022/FR-023, extending Phase 1's
  existing backup/export and Activity audit mechanisms to cover this feature's new Redis entities
  (previously entirely unaddressed — a real data-loss/auditability gap per constitution Principle
  I, not a stylistic one). This pass followed direct user questioning into prefetch/batching/
  prompt-caching mechanics (research.md §15), which surfaced and resolved a persistence-model
  reversal (§16, no new FR needed — a data-model/architecture correction, not a new user-facing
  requirement) before the backup/Activity gap was raised. An LLM-call-specific observability need
  (token usage/cache-hit rate/estimated cost in Activity) was identified as beyond this feature's
  own scope during this discussion and tracked in GitHub issue #100 rather than folded into
  FR-023. FR count grew from 27 to 29; no checklist item changed pass/fail state.
- 2026-09-06 (fourth pass, same day) a direct follow-up question ("does the translated-image cache
  share quota with an existing cache?") surfaced that this spec's own supporting documents
  (research.md §6, data-model.md, tasks.md T020) had never actually verified their "alongside
  Phase 1's existing thumbnail cache" claim against the real code — the thumbnail cache has no
  quota/eviction mechanism at all; the reader's resize-page cache (`tempmaxsize`) does, and is the
  correct match. Corrected in research.md (new §18), data-model.md, and tasks.md; no FR/SC change
  — a factual correction to an implementation detail this checklist's own "no implementation
  details leak into specification" criterion was never meant to police at that level of
  granularity in the first place, so no checklist item's pass/fail state was ever at stake here.
- Items marked incomplete require spec updates before `/speckit-clarify` or `/speckit-plan`.
