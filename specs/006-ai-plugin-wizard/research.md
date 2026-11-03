# Research: AI Plugin Creation Wizard

## 1. DeepSeek tool-calling support and model choice

**Decision**: Extend `lanrurugi-llm` with a new `tool_chat()` function that sends a full `messages`
array plus a `tools` array to DeepSeek's chat-completions endpoint, using model `deepseek-v4-flash`
(switched from the originally-planned `deepseek-v4-pro`, 2026-08-25 — see the model-choice rationale
below for why, and `crates/lanrurugi-llm/src/lib.rs`'s own `DEEPSEEK_MODEL` doc comment, the current
authoritative source, for why `-pro` was dropped in practice). The existing `chat()`/`json_chat()`
(single system+user turn, no tools) keep their own call shape unchanged for their existing callers
(`tankoubons.rs::ai_rename_suggestions`, `recommend_llm.rs`, `artist_backfill.rs`) — only the model
name they resolve to changed, consolidated onto the same `DEEPSEEK_MODEL` constant every call site
now shares (2026-08-25, explicit user direction to standardize every LLM call site on `-flash`).

**Rationale**:
- DeepSeek's API supports OpenAI-compatible tool/function calling: a `tools: [{type: "function",
  function: {name, description, parameters}}]` request field, a `tool_calls` array on the assistant
  response message when the model wants to invoke one, and conversation continuation by appending a
  `{role: "tool", tool_call_id, content}` message and re-sending the full history (verified against
  DeepSeek's own `/guides/tool_calls` API docs). This maps directly onto spec FR-010's "system
  executes — AI judges — proposes the next request" loop: the wizard's `fetch_page` capability
  becomes a tool the model can call zero or more times before returning final plugin code, with the
  system executing the fetch locally each time and feeding the (redirect-trail-inclusive, per FR-011)
  result back as a tool message.
- **`deepseek-chat`/`deepseek-reasoner` (the model names the existing `lanrurugi-llm::chat()` hard-
  codes) were announced discontinued 2026-07-24, i.e. already past as of this plan's 2026-08-24 date**
  — confirmed by DeepSeek's own changelog (`/updates/`), cross-checked via two independent fetches
  after an initial page-summary hallucinated an unrelated "built on Anthropic's Claude Agent SDK"
  claim that didn't survive a second, more targeted fetch. In practice the old names still work today
  (user-confirmed live) because DeepSeek is currently aliasing them to `deepseek-v4-flash`'s non-
  thinking/thinking modes during a transition window, but that's an implementation detail of
  DeepSeek's own migration, not a guarantee — a brand-new feature being built *now* has no reason to
  target a name whose own vendor has already announced its retirement. `deepseek-v4-pro` (GA'd
  2026-08-13, 11 days before this plan) is used instead for the wizard's own new call sites.
- `deepseek-v4-pro` (not `-flash`) specifically for the *generation* path: this is a genuine code-
  generation task (real TypeScript against a real page-structure understanding, chained through a
  multi-step tool-calling loop), where reasoning quality has a direct, visible effect on whether the
  generated selector logic actually works — worth the extra cost/latency over `-flash` for this
  particular call site. The existing `chat()`/`json_chat()` callers (tag suggestions, rename
  suggestions) are simpler classification/formatting tasks and are deliberately left on their current
  model resolution unchanged by this feature (out of scope — a pre-existing "these names still work
  for now" situation this plan does not need to fix to deliver 006, though it's worth a follow-up
  issue independent of this feature).
- The existing `chat()`'s markdown-fence-stripping and DeepSeek-specific error-code translation
  (401/402/429/5xx → localized messages) are genuinely reusable logic, not tied to the single-turn
  shape — `tool_chat()` factors the HTTP-call/error-handling body into a shared private helper both
  functions call, rather than duplicating it (per constitution's "near-identical logic... factored
  into a shared helper" rule).

**Alternatives considered**:
- *Keep using `deepseek-chat` for the new call sites too, matching existing code*: rejected — this
  plan is the first place a *new* model reference gets written, and writing a name the vendor has
  already announced discontinued (whether or not it currently still resolves) is choosing a known
  liability out of pure precedent, precisely the kind of preventable retrofit-later cost the
  "verify at implementation time, not from memory/precedent" dependency discipline this project
  already applies to crate versions is meant to avoid.
- *A dedicated Rust agent-framework crate (e.g. `rig`, `swiftide`) for the tool-calling loop*:
  rejected — the loop itself is a handful of lines (call → check `tool_calls` → execute → append →
  repeat until none), and this project has exactly one tool (`fetch_page`) with one call site; a
  framework's abstraction cost (new dependency, new concepts, version-churn risk) isn't justified for
  a loop this small, matching this project's existing bias toward direct `reqwest` calls over heavier
  SDKs elsewhere in `lanrurugi-llm`.

## 2. Draft trial-run execution path

**Decision**: Stage a draft's code to a temporary file under `plugins/custom/_wizard/<uuid>.ts`
before every trial run, call the existing `PluginPool::plugin_info()`/`execute()` against that
staged namespace exactly as `crates/lanrurugi-api/src/plugins.rs::upload_plugin` already does for a
real upload, then always delete the staging file afterward — regardless of whether the trial run
succeeded or failed. Nothing is ever promoted out of `_wizard/` automatically; only an explicit
confirm-save (FR-020) writes to a real `plugins/<category>/` directory, reusing `upload_plugin`'s
own validate → move → rollback-on-failure logic for that step.

**Rationale**:
- `PluginPool` has no "run this code string directly" API — `plugin_info(namespace)`/
  `execute(namespace, ...)` both resolve `namespace` to a real file at `plugins_dir.join(namespace +
  ".ts")` (confirmed by reading `pool.rs`). A temp-file-per-trial-run is therefore not a workaround,
  it's the only path the existing, already-hardened two-phase-permission sandbox model exposes —
  which is exactly what spec FR-015 (draft trial runs isolated to the same degree as real plugins)
  and the earlier architecture decision ("reuse the Deno sandbox, don't build a second one") call for.
- `upload_plugin` already solves "land untrusted `.ts` content on disk, probe it safely, then decide
  what to do with it" end to end, including the zero-permission `plugin_info()` probe before a real
  run and rollback-by-deletion on any failure. The wizard's trial-run path is a variant of that same
  flow with one difference: the terminal action isn't "move to a category directory", it's "always
  delete", since a trial run is validation, not installation.
- `_wizard/` is a distinct subdirectory from `custom/`'s normal staging use (per `upload_plugin`'s
  own `CUSTOM_PLUGIN_DIR` constant) specifically so a trial-run file can never be confused with, or
  accidentally left behind as, a real uploaded-but-not-yet-approved plugin — the two staging
  lifecycles (upload's "stage → validate → promote" vs. the wizard's "stage → run → always discard")
  are different enough to warrant not sharing a directory, even though both sit under `custom/`.
  `discover_namespaces`' recursive scan will technically walk into `_wizard/` like any other
  subdirectory, but nothing in the existing plugin-listing UI ever calls `list_plugins()` mid-trial-
  run in a way that would surface a transient file to a user — the window where a staged file exists
  on disk is the duration of one `execute()` call, deleted in a `finally`-equivalent (Rust: a guard/
  `Drop`, or explicit cleanup on every return path) immediately after.
- Multi-link trial runs (metadata/download types, FR-014, at least 3 links) reuse the *same* staged
  file for all three `execute()` calls — the code doesn't change between links, only the `hostArgs`
  URL passed to `execMetadata`/`execDownload` does, so staging happens once per trial-run request,
  not once per link.

**Alternatives considered**:
- *A genuinely separate, lighter-weight sandbox for drafts (e.g. a stripped-down Deno invocation with
  no permission-probe phase)*: rejected in the earlier architecture-decision round (confirmed again
  here) — the whole point of "reuse the Deno sandbox" was to inherit its already-audited two-phase
  permission model and per-namespace process isolation for free; a second, parallel execution path
  would need to independently re-earn that same safety bar, doubling the surface this project has to
  keep correct for no real benefit (a draft is exactly as untrusted as a freshly-uploaded plugin, so
  it deserves exactly the same treatment, not a lighter one).
- *In-memory execution via a Deno API that accepts source text directly, bypassing the filesystem*:
  investigated and rejected — `PluginPool`'s subprocess-per-namespace model reads the module path off
  disk inside the *Deno* process (via `import()`), not something the Rust host hands over as a string;
  building an in-memory alternative would mean forking the dispatcher's own import mechanism for this
  one caller, which is a materially bigger, riskier change than writing a temp file the existing path
  already knows how to consume.

## 3. Session/draft-history state location

**Decision**: All `Wizard Session`/`Draft Revision`/`Trial Run Result` state (per spec's Key
Entities) lives entirely in frontend React state (`useWizardSession.ts`, a plain reducer/state hook
in the new `PluginWizard/` page directory). Every backend endpoint the wizard calls is stateless: one
request carries everything needed to produce one result (e.g. a "generate" call includes the page-
feature description, any prior failed code + error for an AI-fix round, and any auxiliary reference
URLs; a "trial-run" call includes the code and the link(s)/credentials to test against). No new
Redis key namespace, no server-held session object, no session-expiry/cleanup job to write.

**Rationale**:
- Directly matches the spec's own stated Assumption: "history only needs to survive the session's own
  lifetime... no requirement to persist across sessions (e.g. a browser refresh)." A frontend-only
  state model satisfies this exactly, with zero extra infrastructure.
- The agentic tool-calling loop within one "generate" call (FR-010's loop) is fully self-contained
  inside a single backend request — the Rust handler itself loops (call DeepSeek → check for
  `tool_calls` → execute `fetch_page` locally → append the tool result → call again → ... → return
  once the model produces final code, no more tool calls), so the loop's own intermediate steps never
  need to be visible across multiple HTTP round-trips to the frontend at all (matching FR-010's "the
  loop's intermediate process is not required to be visible to the user in real time" wording
  precisely — it isn't just permitted to be hidden, this design makes it structurally impossible for
  the frontend to observe mid-loop state even if it wanted to, which is the simplest way to guarantee
  the requirement holds).
- Keeps the four Key Entities from the spec (Wizard Session, Domain Lookup Result, Draft Revision,
  Trial Run Result) as pure frontend data shapes with no corresponding Rust struct/Redis schema to
  design, version, or migrate — there is genuinely nothing to persist, so there's nothing here for
  data-model.md's entity section to define beyond the frontend TypeScript shapes (captured in
  contracts/ instead, alongside the request/response shapes of the endpoints that produce them).

**Alternatives considered**:
- *A short-lived server-side session (e.g. Redis with a TTL, keyed by a wizard-session ID the
  frontend carries)*: rejected — this would add real infrastructure (a new Redis namespace, a TTL/
  cleanup policy, a session-not-found error path) to satisfy a requirement ("survives one page
  session") that a plain in-memory frontend model already satisfies for free, and would contradict
  the spec's own explicit assumption that cross-refresh persistence is *not* required.

## 4. Redirect handling (FR-011)

**Decision**: A dedicated `fetch.rs` helper builds a `reqwest::Client` with a custom
`redirect::Policy::custom` closure that (a) records every URL visited via `attempt.previous()` into a
`Vec<Url>` trail, and (b) returns `Action::stop()` once the trail length exceeds a fixed cap,
surfacing "redirect cap exceeded" as a distinct, explicit outcome (not a generic network error) so
the tool result handed back to AI can say so plainly, per FR-011's "supply the fact that the cap was
exceeded" wording. Client construction happens once (not per-request) and is reused by both the
`generate` step's `fetch_page` tool and the `trial-run` step's own per-link fetch execution.

**Rationale**: `reqwest`'s built-in `redirect::Policy::limited(n)` caps hop count but discards the
intermediate trail (only the final response/URL survives); FR-011 explicitly requires the *full*
trail (original address, every intermediate hop, final address), not just pass/fail on the cap, so
the built-in policy alone is insufficient and the `custom` variant (which `reqwest` exposes for
exactly this kind of case) is needed regardless of the cap value chosen.

**Alternatives considered**:
- *Manually looping HTTP requests with redirects disabled, following `Location` headers by hand*:
  rejected — `reqwest`'s own `redirect::Policy::custom` already provides trail visibility without
  reimplementing redirect-following (cookie jar continuity across hops, method/body handling per the
  various 3xx codes, etc.) by hand, which is exactly the kind of already-solved problem not worth
  re-solving.

## 5. Code editor dependency versions

**Decision**: `@uiw/react-codemirror` (React wrapper), `@codemirror/lang-javascript` (TypeScript
highlighting via `javascript({ typescript: true })`), `codemirror` (bundled basic-setup meta-
package). Verified latest stable versions as of this plan (2026-08-24, `npm view <pkg> version`):
`@uiw/react-codemirror@4.25.11`, `@codemirror/lang-javascript@6.2.5`, `codemirror@6.0.2`. These are
the versions Setup tasks in tasks.md should pin — per constitution's dependency-version rule, this is
a starting candidate to reverify at actual implementation time, not a hard pin to match exactly if a
newer stable release has since shipped.

**Rationale**: This project has never previously integrated a code-editing component (confirmed
during the earlier architecture-decision research pass — no Monaco/CodeMirror/Ace anywhere in
`apps/frontend`), so there's no existing convention to match, only a real requirement: single-file
TypeScript syntax highlighting in a React app. `@uiw/react-codemirror` is the de facto standard React
wrapper for CodeMirror 6 (avoids hand-rolling CodeMirror's own imperative view-lifecycle management
inside a React component, which is a well-known, error-prone integration seam CodeMirror 6's own docs
call out); Monaco was the other option seriously considered in the earlier architecture round and
rejected there for being heavier than this single-file, no-IntelliSense use case needs.

**Alternatives considered**: See the earlier architecture-decision conversation (Monaco Editor vs.
CodeMirror 6 vs. plain `<textarea>` MVP) — CodeMirror 6 was the user's explicit choice; this research
step only pins down which concrete packages/versions implement that choice.

## 6. Timeout values

**Decision**: Three internal bounds, all framed as safety limits against a hung request — not as a
precise commitment to SC-004's "reasonable wait" (which plan.md's own Performance Goals section
deliberately leaves unquantified, since LLM response latency is an external variable this plan
doesn't control):

- **Per-`fetch_page` HTTP call** (`fetch_with_redirect_trail`, research.md §4): 10 seconds
  (`reqwest::ClientBuilder::timeout`). A `fetch_page` tool call is one plain HTTP GET against a
  page/reference URL the user themselves supplied as something they expect to be reachable; 10s is
  generous for a normal page load while still failing fast enough that one slow/hanging target site
  doesn't stall an entire generation round waiting on a single tool call.
- **Per-`tool_chat()` LLM call** (`crates/lanrurugi-llm::tool_chat`): 60 seconds
  (`reqwest::ClientBuilder::timeout` on the client `tool_chat` uses, separate from `fetch_page`'s
  client). Generation is a genuine code-generation task, potentially with `deepseek-v4-pro`'s own
  extended reasoning before it emits a response — longer than a typical chat completion, but still
  bounded, since an unbounded LLM call is indistinguishable from a hung request from the caller's
  perspective either way.
- **Whole `generate` request (the full agentic loop, research.md §1)**: 120 seconds total, wrapping
  the entire while-loop in `generate.rs` in one `tokio::time::timeout`. The loop can make multiple
  `tool_chat()` calls (each individually capped at 60s above) plus multiple `fetch_page` calls (each
  capped at 10s) — without an outer bound, a model that keeps calling `fetch_page` without ever
  converging on final code could in principle run for an unbounded number of rounds. 120s permits a
  handful of tool-calling round-trips (realistically 1-3 fetches is the common case) while still
  giving up well before a user would reasonably conclude the wizard is simply broken.
- **Trial-run's own `PluginPool::execute()` calls** (`crates/lanrurugi-plugin/src/pool.rs`): no new
  timeout — reuses the existing `DEFAULT_TIMEOUT = Duration::from_secs(30)` constant
  (`pool.rs:29`) every other plugin invocation in this codebase is already bound by. A draft trial
  run gets exactly the same time budget a real, installed plugin's own `execMetadata`/`execDownload`/
  `execLogin` call already has — introducing a different value here would be an unexplained,
  draft-specific special case with no motivating reason.

**Rationale**: All four numbers exist to bound worst-case latency (a hung external call, an
unproductive tool-calling loop) rather than to hit a specific target — consistent with plan.md's own
position that SC-004 has no fixed millisecond number because LLM latency isn't something this plan
controls. Reusing `PluginPool`'s existing 30s constant for trial runs, specifically, avoids inventing
a second, parallel timeout policy for what is otherwise the exact same execution path real plugins
already go through.

**Alternatives considered**:
- *No explicit timeouts, relying on the HTTP client's platform default*: rejected — `reqwest`
  has no default request timeout at all (unbounded by default), which is precisely the "stuck, no
  feedback" failure mode SC-004 exists to rule out; an explicit, if generous, bound is required.
- *A single shared timeout constant for all three new call sites*: rejected — a page fetch, an LLM
  call, and a multi-round agentic loop are different operations with genuinely different expected
  durations; collapsing them to one number would make it either too tight for the slowest (the full
  loop) or too loose for the fastest (a single page fetch) to actually catch a hang promptly.
