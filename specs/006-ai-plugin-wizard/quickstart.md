# Quickstart: Validating the AI Plugin Creation Wizard

Prerequisites: Phase 1 (`001-lanrurugi-full-rewrite`) built and running with an LLM API key
configured (Settings page, or `DEEPSEEK_API_KEY` env var per `lanrurugi-llm::resolve_api_key`); this
feature only adds to the existing `lanrurugi-server` binary and `apps/frontend` app, no new
deployable. A local fixture site (a small static HTTP server serving 3+ pages with a stable, known
structure — see research.md's testing note) is recommended over a real external site for repeatable
validation of US1–US6; US7's login-detection path additionally needs a fixture page that returns a
401/403 or redirects to a login page when accessed without a session cookie.

## 1. Domain lookup reports accurate coverage (US1)

```
# In the UI: Plugins page → "Create with AI" → enter a domain that already has one installed
# plugin (e.g. an existing metadata plugin's own url_pattern-matched domain) but is missing the
# other two types
```

**Expected**: the lookup screen shows that type as covered (with which plugin it maps to) and marks
it unselectable, while the other two types are shown as missing and selectable — confirm the exact
shape via `POST /api/plugin-wizard/lookup` directly too (`contracts/plugin-wizard-api.md`).

## 2. Fully-covered domain offers nothing to create (US1, edge case)

Look up a domain where all three types already have an installed plugin.

**Expected**: the wizard states "this domain already has full plugin coverage, nothing to create"
and shows no selectable generation target (FR-003).

## 3. Generation succeeds and the loop fetches the real page (US2)

Select "metadata" for a domain with no existing plugin, supply a page-feature description plus 3
distinct fixture-site links, and submit.

**Expected**: a complete `.ts` draft comes back with a correct `pluginInfo()`/`execMetadata()`
shape; confirm via server logs (or a temporary debug trace) that the `generate` call's tool-calling
loop actually invoked `fetch_page` against at least one of the supplied links before returning final
code — this is the concrete signal that AI is grounding generation in the real page rather than
guessing purely from the text description (US2 AC2).

## 4. Multi-link trial run surfaces per-link results (US3)

Trial-run the draft from step 3 against all 3 supplied links, where one of the fixture pages has a
deliberately different HTML structure than the other two.

**Expected**: `POST /api/plugin-wizard/trial-run` (and the UI) shows independent success/failure per
link, not one aggregate verdict — the differently-structured page fails while the other two succeed,
each with its own result (US3 AC4).

## 5. A trial run never touches the real plugin list (US3, edge case)

Before and after running step 4's trial run, call `GET /api/plugins/metadata` (the existing plugin-
listing endpoint).

**Expected**: the list is identical before and after — the trial run's staged file under
`plugins/custom/_wizard/` never appears (research.md §2).

## 6. Manual edit + re-run (US4)

Edit the draft's code directly in the wizard's editor (e.g. fix an obviously wrong selector) and
trigger a new trial run without leaving the page.

**Expected**: the edit is reflected in a new draft revision; the previous revision's trial-run
history is still visible in the history panel, unmodified (US4 AC1).

## 7. AI auto-fix, then hitting the cap (US5)

Deliberately submit a page-feature description likely to produce a failing selector, trial-run it,
then click "AI auto-fix" repeatedly.

**Expected**: each auto-fix round appends a new revision and auto-triggers its own trial run; after
the 3rd consecutive auto-fix for this type, the action becomes disabled with a clear "cap reached"
message, and the type's revision history (all rounds, not just the last one) remains fully visible
and selectable (US5 AC2/AC3, FR-018).

## 8. Confirm-save installs a real, callable plugin (US6)

From any revision with at least one trial-run result, click confirm-save.

**Expected**: `GET /api/plugins/metadata` now lists the new plugin; it can subsequently be invoked
like any hand-written or `upload_plugin`-uploaded one through the normal plugin-execution paths —
confirm by triggering whatever real feature (e.g. a Library-page metadata refetch) would call a
metadata plugin for a matching URL.

## 9. Namespace conflict is rejected, not overwritten (US6, edge case)

Attempt to save a draft whose declared namespace matches an already-installed plugin.

**Expected**: `409` from `POST /api/plugin-wizard/save`, the existing plugin file is byte-for-byte
unchanged (diff it before/after), and the wizard lets the user rename and retry (FR-021).

## 10. A 403 leads to an AI-suggested login plugin, then a working association (US7)

Generate and trial-run a metadata/download draft against the fixture page that requires a login
session (step 0's prerequisite); do **not** select the login type up front.

**Expected**:
1. The trial run's failing link surfaces a `login_suggestion.relevant: true` verdict (not from a
   local 403-keyword check — verify by checking the actual reasoning text came back from the LLM
   call, not a hardcoded string).
2. Accepting the suggestion walks the user through generating (or, if a login plugin already exists
   for this domain, directly reusing) a login plugin.
3. Once that login plugin passes its own trial run, the originally-failing draft is automatically
   updated with a `login_from` association declaration, and re-trial-running it against the
   previously-failing link now succeeds (US7 AC1–AC5).

## 11. Credentials never reach the LLM (US7 / FR-012, security-critical — verify directly, don't trust the UI alone)

While running step 10, capture the actual HTTP request bodies this feature sends to
`api.deepseek.com` (e.g. via a local proxy, or temporary request logging in `lanrurugi-llm`).

**Expected**: the test account/password entered for the login type's trial run appears in **zero**
of the captured LLM request bodies, at any point in the flow — not in `generate`, not in the
auto-fix path, not in the login-relevance-judgment call. Only the sanitized outcome ever reaches AI.
This check should be run at least once during implementation, not assumed from reading FR-012 alone
(constitution's "verified in a real browser/real call session, not eyeballed" verification
discipline applies here to a backend request, not just frontend UI, for the same reason: a security
property this consequential needs to be observed actually holding, not just believed to follow from
the code as written).
