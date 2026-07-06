# Phase 2 Design Notes (Exploratory — Not a Spec, Not a Plan)

**Status**: Exploratory technical notes captured from design discussion, 2026-07-05. This is
**not** a spec-kit `spec.md`/`plan.md` artifact and carries no governance weight — it exists so
the design ideas below survive until Phase 2 gets its own formal `/speckit-specify` +
`/speckit-plan` cycle, per constitution Principle VI ("Phase 2 ... MUST NOT be smuggled into
Phase 1 specs/plans/tasks, and MUST NOT block or gate Phase 1 delivery"). Nothing here is
committed, reviewed against a quality checklist, or ready for `/speckit-tasks`. It corresponds to
the high-level intent already captured formally in `spec.md`'s User Stories 9–10
(FR-023–FR-027, SC-006–SC-007) — this document is the "how we were thinking of building it"
layer underneath that "what/why."

## 1. OCR pipeline

- **Batching**: OCR detection network (e.g. a DBNet-style model) should take a dynamic batch size
  (4 or 8 pages per batch is the working assumption) rather than one page at a time, to use
  hardware matrix-parallelism properly.
- **IoU/geometric line merging**: raw OCR output is scattered single-column text boxes. Boxes
  that overlap heavily (high IoU) or sit within a small horizontal/vertical distance threshold of
  each other should be merged into one paragraph box, so the eventual LLM translation step gets a
  coherent block of text instead of fragments — this matters for translation quality, not just
  display.

## 2. Volume-level font-lock cache (three-stage state machine)

**Goal**: avoid calling an expensive font-recognition model (e.g. a YuzuMarker-style classifier)
on every single text slice; most of a given manga volume uses a small, consistent set of fonts.

**Stage 1 — Voting phase**: for the first ~1–3 pages, every text block is cropped
(in-memory, via Rust image cropping) and run through the full font classifier. Results accumulate
in an in-memory histogram (`HashMap<FontName, u32>`).

**Stage 2 — Locking phase**: once total votes exceed a threshold (working number: 40), take the
top 2–3 dominant fonts as the volume's "golden font set" and mark state as locked. After locking,
new text blocks should **not** go through the heavy crop+classify path — instead a cheap CPU-only
feature check (word count, bubble aspect ratio, punctuation presence) routes each block to one of
the 2–3 golden fonts.

**Stage 3 — Meltdown fallback**: even while locked, if a block's cheap features are extreme
outliers (unusually large font size, a flashback page rendered in a different serif, etc.), force
a one-off full classifier re-inference rather than trusting the cheap router, and *evaluate*
(not unconditionally commit) whether to update the cache pool from that result.

### Open design concerns raised during review (unresolved — flag for Phase 2 planning)

A rough mermaid sketch of this flow surfaced three issues worth resolving before this becomes a
real plan:

1. **Meltdown results must not blindly feed the same vote pool used during Stage 1.** If an
   outlier/meltdown re-inference (e.g. a recurring flashback font) is added to the same histogram
   used to pick the golden set, repeated meltdowns can drift the golden-font ranking over time and
   undermine the entire point of locking. Needs either a separate tracking mechanism for
   meltdown results, or an explicit, conditional (not automatic) pool-update rule.
2. **The "is this cached/locked" check and the "does this block's cheap features look normal"
   check are two logically different decisions** (one reads persistent per-volume state, one runs
   a per-block classifier) and should probably be modeled/implemented as two sequential checks,
   not one combined branch — mainly an implementation-clarity concern, not a correctness bug.
3. **Fast-path font dispatch must route among the top 2–3 golden fonts using the cheap features,
   not always dispatch a single Top-1 font.** A rough version of the flow collapsed this to
   "always Top-1," which would lose legitimate intra-page font variety (dialogue vs. sound effects
   vs. narration commonly use different fonts even within one "normal" volume). Confirm this was
   just a sketch simplification, not an intended behavior change, when this gets formally planned.

## 3. Translation backend abstraction

- **Provider surface exposed to the user**: OpenAI / Anthropic / Ollama, user-selectable.
- **Actual adapter count**: effectively **two** implementations, not three — Ollama is exposed as
  a preset of the OpenAI-compatible adapter pointed at Ollama's own `/v1` OpenAI-compatible
  endpoint (with a placeholder API key), rather than a bespoke third client. Anthropic gets its own
  adapter (distinct `system` field, content-block array, `x-api-key`/`anthropic-version` auth,
  mandatory `max_tokens`, distinct SSE event shape — already captured in the constitution's
  Technology Stack Constraints for when Phase 2 is built).
- **Trust-boundary split (already binding, see constitution Principle V)**: cloud providers
  (OpenAI-compatible, Anthropic) are called **server-side**, keeping API keys off the browser
  entirely, since there's no reason to expose them once we've established the server can freely
  reach the public internet. Local models (Ollama on the *browser's* own machine) are the one
  case that must originate from the browser, since the server has no route to the client's
  loopback interface.
- **Local-model browser connectivity (Private Network Access)**: mixed content is not the actual
  blocker (loopback targets are exempt); Chromium's Private Network Access is the real
  constraint, and it is scheme-independent (applies to plain HTTP too) — it fires whenever the
  page's origin is classified public/private and the fetch target is loopback. Options ranked by
  user-friction, cheapest first:
  1. **Configure Ollama itself** (`OLLAMA_ORIGINS` +, if/when supported, responding to the PNA
     preflight with `Access-Control-Allow-Private-Network: true`) — zero extra installation, since
     the user already runs Ollama. Preferred default; verify current Ollama support at
     implementation time.
  2. **A local companion proxy/bridge** shipped as a subcommand of the same LANrurugi binary
     (not a separate program to download) — only if (1) isn't sufficient for a given user's setup.
  3. **A browser extension** — smoother than a raw binary+terminal bridge (no manual run step,
     auto-starts with the browser) but still an install; last resort.
  - Explicitly rejected: telling users to disable browser security features — bad practice for a
    public project to recommend, not a real solution.
  - UI should feature-detect (try direct connection, fall back to guidance) rather than assume
    any one path always works, since this is genuine, evolving browser-vendor policy territory.

## 4. Reader-side rendering (sliding-window prefetch)

- Frontend prefetches OCR layout + clean (text-removed) page images for N+1..N+3 pages while the
  user reads page N.
- Burn-in rendering of translated text happens **client-side** (Canvas/SVG overlay in the
  browser), not server-side — meaning the constitution's CJK server-font-bundling requirement is
  unrelated to this feature; it depends on the browser's own font stack, not the server image's
  fonts.
- Look-ahead depth/aggressiveness should default conservatively for metered (paid) translation
  backends and can be more aggressive by default only for zero-marginal-cost local backends —
  already codified as a binding rule in the constitution's Engineering Workflow & Quality Gates
  ("Cost- and rate-limit-aware defaults"), reiterated here because it directly shapes how
  aggressive this prefetch window can be per backend.

## Non-goals for this document

This is not an approved architecture. Concurrency/perf targets, exact model choices, exact crate
choices, and UI details are all open until Phase 2 goes through its own `/speckit-specify` →
`/speckit-clarify` → `/speckit-plan` cycle.
