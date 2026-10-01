# Data Model: AI Plugin Creation Wizard

Per research.md §3, none of this feature's entities are persisted server-side — everything below is
a frontend TypeScript shape held in `PluginWizard/useWizardSession.ts`'s state, reconstructed fresh
each time the wizard is opened. The only artifact that outlives a wizard session is the final `.ts`
file written by confirm-save (FR-020), which becomes an ordinary plugin file with no wizard-specific
shape of its own — it's indistinguishable on disk from a hand-written or `upload_plugin`-uploaded one.

## WizardSession

The root state object for one use of the wizard.

| Field | Type | Notes |
|---|---|---|
| `domain` | `string` | The domain the user entered and looked up (FR-001). Immutable once the lookup succeeds; changing it starts a brand-new session (per spec Edge Cases: switching types/domain mid-session invalidates prior history for the old target). |
| `lookupResult` | `DomainLookupResult` | See below. Populated once, right after `POST /plugin-wizard/lookup` returns. |
| `selectedTypes` | `PluginType[]` | Subset of the types `lookupResult` reported missing (FR-004); 1–3 entries. |
| `typeSessions` | `Record<PluginType, TypeSession>` | One entry per selected type — see below. This is where almost all of the wizard's real state lives. |

## DomainLookupResult

The output of one domain lookup (FR-002/FR-003), one entry per plugin type.

| Field | Type | Notes |
|---|---|---|
| `login` | `TypeCoverage` | |
| `metadata` | `TypeCoverage` | |
| `download` | `TypeCoverage` | |

```ts
type TypeCoverage =
  | { covered: false }
  | { covered: true; namespace: string; sourceCode: string } // sourceCode: FR-009's reference sample
```

`sourceCode` is fetched once at lookup time (not re-fetched per generation call) and threaded through
to `generate` requests for other types on the same domain as the FR-009 reference sample — this is
plain data flow through `WizardSession`, not a second round-trip to disk at generation time.

## LoginParameter

One credential field a target site's real login mechanism actually needs, as determined by
`POST /plugin-wizard/analyze-login` inspecting the real login page/API doc (not assumed up front
to be an account+secret pair — could be a single token/API key, a raw cookie value, or something
else). Mirrors `PluginInfoResult.parameters`'s own shape exactly, since AI is required to declare
this list verbatim when generating the login plugin afterward.

| Field | Type | Notes |
|---|---|---|
| `name` | `string` | Field identifier (lowercase snake_case, e.g. `account`, `api_key`, `cookie`) — also the key `TypeSession.loginFieldValues` and `Credentials.fields` (the `trial-run` request body) use for this field. |
| `description` | `string` | User-facing label (in Chinese, per this project's own working-language convention for AI-facing prompts). |
| `required` | `boolean` | Whether this field must be filled before `isFormComplete` considers the login type's form ready for generation. |

## TypeSession

Per-target-type state — one of these exists for every entry in `WizardSession.selectedTypes`.

| Field | Type | Notes |
|---|---|---|
| `type` | `PluginType` | `"login" \| "metadata" \| "download"` |
| `testLinks` | `string[]` | Metadata/download only — ≥3 entries required before a trial run can be triggered (FR-005, FR-014). Empty/unused for `login`. No free-text page-feature description field exists (removed after initial implementation, per a UI-simplification pass) — AI is instructed to `fetch_page` these links itself and infer selectors from the real returned HTML instead of from a user-written description. |
| `loginReferenceUrl` | `string` | Login only — the login page/API-doc URL `POST /plugin-wizard/analyze-login` inspects to determine `loginParameters`. |
| `loginParameters` | `LoginParameter[] \| null` | Login only — set once `analyze-login` succeeds; `null` beforehand (no manual-entry fallback exists — analysis must run first, a design change made after initial implementation per real user feedback that a hardcoded account+secret pair is wrong for token/API-key/cookie-authenticated sites). Drives the credential-field form's dynamic rendering. |
| `loginFieldValues` | `Record<string, string>` | Login only — user-entered values keyed by each `LoginParameter.name`. **Never leaves the browser except inside the one `trial-run` request that needs it** — never attached to a `generate` or `AI auto-fix` request payload (FR-012). |
| `auxiliaryReferenceUrls` | `string[]` | Optional (FR-006); empty array is valid. |
| `dependsOnLogin` | `boolean \| null` | Metadata/download only; `null` until explicitly answered (FR-007) — the UI must not default this to `false` or `true` silently. Always `null`/unused for `login` itself. |
| `loginAssociation` | `{ namespace: string; source: "generated-this-session" \| "existing" } \| null` | Set once a login dependency is actually established — either by the FR-013 generation-order path or the FR-025 after-the-fact "add a login plugin" path. `existing` means it points at `DomainLookupResult.login`'s namespace without this session having generated anything for it. |
| `revisions` | `DraftRevision[]` | Ordered oldest → newest. Never truncated (FR-019) — even past the FR-018 auto-fix cap, new revisions keep appending. |
| `activeRevisionIndex` | `number` | Index into `revisions` the user currently has open/editing (FR-019 — not required to be the last one). |
| `autoFixAttemptsUsed` | `number` | Consecutive AI-auto-fix count for *this type*, resets only if a manual edit or a fresh manual generation breaks the "consecutive" chain (FR-018's cap is on *consecutive* attempts). Capped display logic (disable the AI-auto-fix action) lives in the UI layer, driven by comparing this against the fixed cap of 3. |
| `savedNamespace` | `string \| null` | FR-020/FR-029 — set once `/plugin-wizard/save` succeeds for this type; the real installed namespace. `null` means still a draft. Once set, the wizard UI replaces this type's draft actions (trial-run/save/edit) with a simple "Installed as `<namespace>`" state — there's nothing further to do with this type in this session. |

## DraftRevision

One versioned snapshot of a type's code, plus what happened when it was trial-run.

| Field | Type | Notes |
|---|---|---|
| `id` | `string` | Client-generated (e.g. `crypto.randomUUID()`), stable identity for React keys / "set as active" references. |
| `code` | `string` | The `.ts` source. |
| `origin` | `"ai-generated" \| "ai-auto-fix" \| "manual-edit"` | How this revision came to exist — purely descriptive/UI-labeling, no behavioral branching depends on it other than auto-fix-attempt accounting above. |
| `createdAt` | `number` (epoch ms) | For ordering/display only — `revisions` array order is the actual source of truth for "which came first". |
| `trialRuns` | `TrialRunResult[]` | Zero or more. A revision can be trial-run multiple times (e.g. re-running after fixing an unrelated link) — each attempt appends, none are overwritten, so a user can see "it failed twice, then I fixed my test link and it passed" as real history rather than a single mutable slot. |

## TrialRunResult

One real call made against one draft revision.

```ts
type TrialRunResult =
  | {
      type: "login"
      outcome: "success" | "failure"
      detail: string // sanitized — never echoes testCredentials back, even on failure (FR-012)
    }
  | {
      type: "metadata" | "download"
      perLink: {
        link: string
        outcome: "success" | "failure"
        // success: whatever execMetadata/execDownload actually returned, shown as-is (US3 AC3)
        data?: unknown
        error?: string
        redirectTrail?: string[] // FR-011 — present whenever at least one redirect occurred
      }[]
      // FR-025's AI login-relevance judgment, only ever populated when `perLink` has ≥1 failure
      loginSuggestion?: { relevant: boolean; reasoning: string }
    }
```

`loginSuggestion` is deliberately optional and only computed for a *failing* metadata/download run —
per spec Edge Cases, AI's login-relevance judgment must distinguish "needs login" failures from other
failure classes (broken link, code bug), and a run with zero failures has nothing to diagnose.

## State Transitions

```text
WizardSession created (domain entered)
  → lookup succeeds → DomainLookupResult populated → user selects 1-3 missing types
  → per selected type: description (+ links/credentials, + optional aux URLs) collected
  → [if both login + a dependent metadata/download type selected: FR-013 ordering — the
     login TypeSession's first revision must reach a successful trial run before the
     dependent type's first `generate` call is even issued]
  → generate → first DraftRevision (origin: "ai-generated") appended, trialRuns: []
  → trial-run → TrialRunResult appended to the active revision's trialRuns
      → on a metadata/download failure with loginSuggestion.relevant === true and no
        existing loginAssociation: US7/FR-025 path offered — accepting it either reuses
        DomainLookupResult's existing login coverage (sets loginAssociation = {source:
        "existing"}) or spins up a new login TypeSession the same way FR-013's up-front
        path does, then sets loginAssociation = {source: "generated-this-session"} once
        that succeeds, then a fresh DraftRevision is generated for the original failing
        type carrying the association declaration
  → user may: edit code directly (new DraftRevision, origin: "manual-edit") →
              re-trial-run; OR request AI auto-fix (new DraftRevision, origin:
              "ai-auto-fix", autoFixAttemptsUsed += 1) → auto-triggered trial-run;
              OR set activeRevisionIndex to any prior revision
  → confirm-save (per type, independently) → POST /plugin-wizard/save with the chosen
    revision's code → real plugin file written; that TypeSession's further edits/fixes
    remain possible (spec doesn't require locking a type after save) but no longer block
    anything else in the session
```
