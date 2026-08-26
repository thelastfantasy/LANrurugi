import { useCallback, useReducer } from "react"

/** Mirrors `specs/006-ai-plugin-wizard/data-model.md` exactly — every entity here is frontend-only
 * state; nothing in this file is ever persisted server-side (research.md §3). */

export type PluginType = "login" | "metadata" | "download"

/** `WizardSession.sharedLinks` intentionally stores raw, unfiltered textarea lines (including
 * blank ones from an in-progress newline) — see `SharedLinksForm.tsx`'s own doc comment on why
 * trimming/filtering on every keystroke there broke the ability to type a newline at all. Every
 * *consumer* of the list (as opposed to the editor itself) calls this once, at the point of use —
 * gating logic (`isFormComplete`/`canSaveFor`) and outbound request bodies alike. */
export function cleanLinks(links: string[]): string[] {
  return links.map((l) => l.trim()).filter(Boolean)
}

/** Real values for a same-domain login plugin's own credential fields, sent to `/plugin-wizard/
 * generate` so a metadata/download generation can authenticate its own `fetch_page` calls (see
 * `generate.rs`'s own docs on `available_credential_fields`/`credential_values`) — the model is
 * told to *assume* these credentials are real and ready to use, not merely possible, whenever this
 * returns non-undefined. Only available for a login `TypeSession` *this session actually
 * generated* (`source: "generated-this-session"`) — an "existing" (already-installed) login
 * plugin's real credential values were never entered into this session's own state at all (no
 * `TypeSession` for it exists), so there's nothing here to send for that case; a metadata/download
 * generation for a domain with no associated login plugin at all (`loginAssociation` absent) gets
 * `undefined` for both, same as before this mechanism existed. */
export function availableCredentialsFor(
  session: WizardSession,
  typeSession: TypeSession,
): { fields: LoginParameter[] | undefined; values: Record<string, string> | undefined } {
  if (typeSession.type === "login" || typeSession.loginAssociation?.source !== "generated-this-session") {
    return { fields: undefined, values: undefined }
  }
  const loginSession = session.typeSessions.login
  return {
    fields: loginSession?.loginParameters ?? undefined,
    values: loginSession?.loginFieldValues,
  }
}

/** Distinguishes what kind of installed plugin a domain-lookup coverage hit actually points at —
 * `namespace`'s own leading path component already carries this (`custom/...` vs. everything
 * else), computed once client-side in `DomainLookupStep.tsx::toTypeCoverage` rather than sent by
 * the backend (no wire-format change needed, `namespace` already has what's needed). Named
 * `coverageSource`, not `source`, on `TypeCoverage` to avoid reading as the same concept as
 * `LoginAssociation.source` (`"generated-this-session" | "existing"`) — a different axis entirely
 * (that one is about *this session's own* login plugin provenance, this one is about *any*
 * installed plugin a lookup happened to match). */
export type CoverageSource = "built-in" | "ai-generated"

export type TypeCoverage =
  | { covered: false }
  | {
      covered: true
      namespace: string
      declaredNamespace: string
      sourceCode: string
      coverageSource: CoverageSource
    }

/** Resolves a `loginAssociation`'s *file-path* namespace — what `usePluginSettings`/
 * `usePluginOptions` actually address a plugin by (`/plugins/settings?namespace=...`,
 * `/plugins/options?namespace=...`), as opposed to `LoginAssociation.namespace` itself, which is
 * always the plugin's own self-declared `pluginInfo().namespace` (see that field's own docs — it
 * has to be the declared one so `login_from` resolution in generated code works, but that's a
 * different namespace than the one settings endpoints key on). `"existing"` resolves via the
 * domain lookup's own already-known file-path namespace; `"generated-this-session"` resolves via
 * this session's own login `TypeSession` once it's actually been saved (unsaved, there's no real
 * installed file yet for a settings endpoint to find). `undefined` if neither source has a
 * resolvable file-path namespace yet. */
export function loginPluginFilePathNamespace(
  session: WizardSession,
  association: LoginAssociation | null,
): string | undefined {
  if (!association) return undefined
  if (association.source === "existing") {
    return session.lookupResult.login.covered ? session.lookupResult.login.namespace : undefined
  }
  return session.typeSessions.login?.savedNamespace ?? undefined
}

export interface DomainLookupResult {
  login: TypeCoverage
  metadata: TypeCoverage
  download: TypeCoverage
}

export interface LoginAssociation {
  namespace: string
  source: "generated-this-session" | "existing"
}

/** One credential field `/plugin-wizard/analyze-login` determined a target site actually needs —
 * could be `account`/`secret` for a password pair, a single `api_key`/`token` field, a `cookie`
 * field, or something else entirely; never assumed to always be a password pair. Mirrors the
 * shape `PluginInfoResult.parameters` already uses. */
export interface LoginParameter {
  name: string
  description: string
  required: boolean
}

export interface TrialRunResultLogin {
  type: "login"
  outcome: "success" | "failure"
  detail: string
}

export interface TrialRunResultLink {
  link: string
  outcome: "success" | "failure"
  data?: unknown
  error?: string
  redirectTrail?: string[]
}

export interface LoginSuggestion {
  relevant: boolean
  reasoning: string
}

export interface TrialRunResultLinks {
  type: "metadata" | "download"
  perLink: TrialRunResultLink[]
  loginSuggestion?: LoginSuggestion
}

export type TrialRunResult = TrialRunResultLogin | TrialRunResultLinks

export interface DraftRevision {
  id: string
  code: string
  /** `"loaded-from-existing"` — the initial revision of an edit-mode session (`editExistingType`),
   * seeded from a previously wizard-saved plugin's own real source code, not freshly generated —
   * same "no AI explanation" treatment as `manual-edit` below. */
  origin: "ai-generated" | "ai-auto-fix" | "manual-edit" | "ai-refine" | "loaded-from-existing"
  createdAt: number
  trialRuns: TrialRunResult[]
  /** User-facing (Chinese) natural-language summary of what this code does — `generate.rs`'s
   * `GenerateResponse.explanation`. Only present on `ai-*`-origin revisions (a manual edit has no
   * AI-authored explanation of its own); absent, not empty-string, for `manual-edit`. */
  explanation?: string
}

/** One past round of this type's generation conversation — see `TypeSession.conversationHistory`'s
 * own doc comment for why this exists and what it's replayed as. */
export interface ConversationTurn {
  userMessage: string
  assistantCode: string
}

export interface TypeSession {
  type: PluginType
  /** Login only — set once analysis succeeds; `null` before that (the credential-field form has
   * nothing to render yet — analysis must run first, no manual-entry fallback). */
  loginParameters: LoginParameter[] | null
  /** Login only — user-entered values for each of `loginParameters`, keyed by `LoginParameter.name`.
   * Never attached to a `generate`/AI-auto-fix request payload (FR-012) — only ever read by the one
   * `trial-run` call that needs it. */
  loginFieldValues: Record<string, string>
  /** Login only — the single URL `analyze-login` inspected and generation/AI-auto-fix should send
   * as `test_links` for this type. Deliberately *not* the same as sending all of
   * `WizardSession.sharedLinks` to generate a login plugin: a login page only needs its own login
   * form inspected, not every metadata/download sample link the user pasted for the other selected
   * types — sending all of them made the AI dutifully `fetch_page` every gallery/detail link too
   * "just in case", burning enough of the 120s generate-timeout budget on irrelevant pages that
   * real generations timed out before producing code (observed live 2026-08-24). Defaults to
   * `sharedLinks`'s first entry in the UI (`TypeDetailsForm.tsx`) but is stored here, not
   * recomputed from `sharedLinks` at request time, so it stays stable even if the user edits
   * `sharedLinks` after already running login analysis against a specific URL. */
  loginReferenceUrl: string
  /** Metadata/download only. `null` until explicitly answered (FR-007) — must never be defaulted
   * to `true`/`false` by the UI. */
  dependsOnLogin: boolean | null
  loginAssociation: LoginAssociation | null
  /** Every past round's own (what was asked, what code came back) pair for this type, oldest
   * first — a fresh generation, an AI-auto-fix, a user-typed refine request, and a manual edit all
   * append one turn each, in the order they actually happened. Sent back to `/plugin-wizard/
   * generate` verbatim as `conversation_history` on every subsequent call for this type, so a
   * later refinement round (or auto-fix, or even a plain re-generate) has the model's own full
   * multi-round context instead of only the single latest code snapshot — added specifically so a
   * user's free-text "在已生成代码基础上追加需求" (`RefinePanel.tsx`) round can build on everything
   * that came before it, not just restate the current code from scratch. */
  conversationHistory: ConversationTurn[]
  revisions: DraftRevision[]
  activeRevisionIndex: number
  /** Consecutive AI-auto-fix count for this type (FR-018's cap is on *consecutive* attempts —
   * a manual edit or fresh generation resets this). */
  autoFixAttemptsUsed: number
  /** Set once `/plugin-wizard/save` succeeds for this type's active revision (US6/FR-020) — the
   * real installed namespace. `null` while still a draft. */
  savedNamespace: string | null
  /** Set alongside `savedNamespace` — the plugin's own self-declared `pluginInfo().namespace`
   * (distinct from the file-path `savedNamespace` above). For a `login`-type session, this is what
   * a *different* type's `loginAssociation.namespace` must reference (T043) — `resolve_declared_
   * namespace`-style lookups match on this field, not the file-path one. `null` while still a
   * draft, and unused/irrelevant for non-`login` types. */
  savedDeclaredNamespace: string | null
  /** Metadata/download only — the real values the backend resolved from the associated login
   * plugin's own persisted Redis settings for its last generate call (`generate.rs::resolve_
   * credentials`, surfaced via the `done` SSE event's `resolved_credential_values`). Used purely to
   * auto-prefill the trial-run parameter panel by field-name match (`TrialRunResult.tsx`) — the
   * user already set these values on the associated login plugin's own settings page in an earlier
   * session; re-typing them here was pure friction (real report, 2026-08-25: "这里还是没读取到已经
   * 设置的key"). Empty when this type has no login association, or the association has nothing
   * saved yet. */
  resolvedCredentialValues: Record<string, string>
  /** Set only by `editExistingType` — the file-path namespace (e.g. `custom/metadata/foosite`)
   * this session's draft was loaded from and must be written back to on save, not a fresh file.
   * `null` for every ordinary generate-from-scratch session. `TrialRunResult.tsx`'s save flow
   * reads this to lock the filename field and attach `allow_overwrite`/`overwrite_namespace` to
   * the `/plugin-wizard/save` request. */
  editingExistingNamespace: string | null
}

export type WizardStep = "typeSelection" | "sharedLinks" | "typeDetail"

export interface WizardSession {
  domain: string
  lookupResult: DomainLookupResult
  selectedTypes: PluginType[]
  typeSessions: Partial<Record<PluginType, TypeSession>>
  /** Which screen is currently shown — replaces the old "render every selected type's whole panel
   * stacked at once" layout (real user feedback, 2026-08-26: "首先从上到下的瀑布式布局就要改掉").
   * There's no `"lookup"` member here — that step corresponds to `session === null` in `index.tsx`,
   * before a `WizardSession` even exists. */
  currentStep: WizardStep
  /** Which selected type's `TypeWizardPanel` is shown while `currentStep === "typeDetail"` — only
   * one panel is ever mounted at a time now; switching between several selected types re-targets
   * this rather than scrolling to a different stacked block. `null` outside `"typeDetail"`. */
  activeType: PluginType | null
  /** Domain-level shared links (one per line) — the single input the whole wizard run works from:
   * target page links for metadata/download's generate+trial-run (≥3 required before either can
   * start, FR-005/FR-014), the default reference URL login analysis inspects, and any purely
   * auxiliary/API-doc links, all at once. Filled once, used everywhere. Replaces three previously
   * separate fields (`TypeSession.testLinks`, `TypeSession.loginReferenceUrl`, and an earlier
   * `sharedAuxiliaryReferenceUrls`) per two rounds of real user feedback: a single URL — e.g.
   * `https://nhentai.net/api/v2/docs` — very often documents auth, metadata structure, *and*
   * download behavior all on one page, and having to paste it into two or three separate boxes
   * (once per selected type, plus again for login analysis) was pure repetition. The backend and
   * AI are responsible for figuring out which of these links are actually useful for which
   * purpose — the user just supplies "everything relevant about this domain" once. */
  sharedLinks: string[]
}

function newTypeSession(type: PluginType): TypeSession {
  return {
    type,
    loginParameters: null,
    loginFieldValues: {},
    loginReferenceUrl: "",
    dependsOnLogin: null,
    loginAssociation: null,
    conversationHistory: [],
    revisions: [],
    activeRevisionIndex: -1,
    autoFixAttemptsUsed: 0,
    savedNamespace: null,
    savedDeclaredNamespace: null,
    resolvedCredentialValues: {},
    editingExistingNamespace: null,
  }
}

type Action =
  | { kind: "lookupSucceeded"; domain: string; result: DomainLookupResult }
  | { kind: "typeSelected"; type: PluginType }
  | {
      // Per spec's own Edge Case on switching types mid-session: deselecting discards the
      // TypeSession entirely (all revisions/trialRuns history included) rather than keeping it
      // around inert. Reselecting the same type later starts a genuinely fresh TypeSession — it
      // never resurrects the discarded one.
      kind: "typeDeselected"
      type: PluginType
    }
  | { kind: "typeSessionUpdated"; type: PluginType; patch: Partial<TypeSession> }
  | { kind: "sharedLinksChanged"; links: string[] }
  | { kind: "revisionAppended"; type: PluginType; revision: DraftRevision }
  | { kind: "conversationTurnAppended"; type: PluginType; turn: ConversationTurn }
  | { kind: "activeRevisionChanged"; type: PluginType; index: number }
  | { kind: "trialRunAppended"; type: PluginType; revisionId: string; result: TrialRunResult }
  | { kind: "reset" }
  | { kind: "advanceToSharedLinks" }
  | { kind: "advanceToTypeDetail"; type: PluginType }
  | { kind: "activeTypeChanged"; type: PluginType }
  | { kind: "stepBack" }
  | {
      // Clicking a step in the "1 查找域名 2 选择类型 3 共享链接 4 生成与保存" indicator itself —
      // real user feedback, 2026-08-26: "这里1到4能点击切换更好". Only ever moves *backward* (or
      // stays put) — jumping forward past a step that isn't complete yet would bypass that step's
      // own gating (e.g. arriving at 生成与保存 without ≥3 shared links entered), so a forward
      // click is simply ignored rather than allowed through; only "下一步" advances forward, same
      // as before this action existed.
      kind: "goToStep"
      step: WizardStep
    }
  | {
      // Domain-lookup hit an `ai-generated`-source coverage entry — jumps straight into edit
      // mode for that type, seeding a real initial revision from the plugin's own already-saved
      // source rather than starting from an empty form (see `TypeSelectionStep.tsx`'s own
      // "编辑已有 AI 插件" button).
      kind: "editExistingType"
      type: PluginType
      namespace: string
      declaredNamespace: string
      sourceCode: string
    }

function reducer(state: WizardSession | null, action: Action): WizardSession | null {
  switch (action.kind) {
    case "lookupSucceeded":
      return {
        domain: action.domain,
        lookupResult: action.result,
        selectedTypes: [],
        typeSessions: {},
        sharedLinks: [],
        currentStep: "typeSelection",
        activeType: null,
      }
    case "reset":
      return null
  }

  if (!state) return state

  switch (action.kind) {
    case "typeSelected": {
      const existing = state.typeSessions[action.type]
      // Already selected and NOT mid-edit — a plain re-select is a no-op. An edit-mode session
      // for this type, though, must fall through and get replaced: 生成覆盖版本 clicked from an
      // ai-generated-covered row means "start fresh instead of editing", not "do nothing" (real
      // user feedback, 2026-08-26: both buttons should be independently clickable side by side).
      if (state.selectedTypes.includes(action.type) && existing?.editingExistingNamespace == null) return state
      return {
        ...state,
        selectedTypes: state.selectedTypes.includes(action.type)
          ? state.selectedTypes
          : [...state.selectedTypes, action.type],
        typeSessions: { ...state.typeSessions, [action.type]: newTypeSession(action.type) },
      }
    }
    case "typeDeselected": {
      const { [action.type]: _discarded, ...rest } = state.typeSessions
      const selectedTypes = state.selectedTypes.filter((t) => t !== action.type)
      // The deselected type may have been the one currently shown in the typeDetail step — retarget
      // to another remaining selected type, or fall back to the typeSelection step if none remain,
      // rather than leaving `activeType` pointing at a TypeSession that no longer exists.
      const wasActive = state.activeType === action.type
      return {
        ...state,
        selectedTypes,
        typeSessions: rest,
        activeType: wasActive ? (selectedTypes[0] ?? null) : state.activeType,
        currentStep: wasActive && selectedTypes.length === 0 ? "typeSelection" : state.currentStep,
      }
    }
    case "typeSessionUpdated": {
      const existing = state.typeSessions[action.type]
      if (!existing) return state
      return {
        ...state,
        typeSessions: {
          ...state.typeSessions,
          [action.type]: { ...existing, ...action.patch },
        },
      }
    }
    case "sharedLinksChanged":
      return { ...state, sharedLinks: action.links }
    case "revisionAppended": {
      const existing = state.typeSessions[action.type]
      if (!existing) return state
      const revisions = [...existing.revisions, action.revision]
      // FR-018: the cap is on *consecutive* auto-fix attempts — an "ai-auto-fix" revision
      // increments the counter, anything else (fresh generation, manual edit) resets the chain.
      const autoFixAttemptsUsed =
        action.revision.origin === "ai-auto-fix" ? existing.autoFixAttemptsUsed + 1 : 0
      return {
        ...state,
        typeSessions: {
          ...state.typeSessions,
          [action.type]: {
            ...existing,
            revisions,
            activeRevisionIndex: revisions.length - 1,
            autoFixAttemptsUsed,
          },
        },
      }
    }
    case "conversationTurnAppended": {
      const existing = state.typeSessions[action.type]
      if (!existing) return state
      return {
        ...state,
        typeSessions: {
          ...state.typeSessions,
          [action.type]: {
            ...existing,
            conversationHistory: [...existing.conversationHistory, action.turn],
          },
        },
      }
    }
    case "activeRevisionChanged": {
      const existing = state.typeSessions[action.type]
      if (!existing) return state
      return {
        ...state,
        typeSessions: {
          ...state.typeSessions,
          [action.type]: { ...existing, activeRevisionIndex: action.index },
        },
      }
    }
    case "trialRunAppended": {
      const existing = state.typeSessions[action.type]
      if (!existing) return state
      const revisions = existing.revisions.map((r) =>
        r.id === action.revisionId ? { ...r, trialRuns: [...r.trialRuns, action.result] } : r,
      )
      return {
        ...state,
        typeSessions: { ...state.typeSessions, [action.type]: { ...existing, revisions } },
      }
    }
    case "advanceToSharedLinks": {
      if (state.selectedTypes.length === 0) return state
      return { ...state, currentStep: "sharedLinks" }
    }
    case "advanceToTypeDetail": {
      return { ...state, currentStep: "typeDetail", activeType: action.type }
    }
    case "activeTypeChanged": {
      if (!state.selectedTypes.includes(action.type)) return state
      return { ...state, activeType: action.type }
    }
    case "stepBack": {
      const prior: Record<WizardStep, WizardStep> = {
        typeSelection: "typeSelection",
        sharedLinks: "typeSelection",
        typeDetail: "sharedLinks",
      }
      return { ...state, currentStep: prior[state.currentStep] }
    }
    case "goToStep": {
      const order: Record<WizardStep, number> = { typeSelection: 0, sharedLinks: 1, typeDetail: 2 }
      if (order[action.step] > order[state.currentStep]) return state
      return { ...state, currentStep: action.step }
    }
    case "editExistingType": {
      const revision: DraftRevision = {
        id: crypto.randomUUID(),
        code: action.sourceCode,
        origin: "loaded-from-existing",
        createdAt: Date.now(),
        trialRuns: [],
      }
      return {
        ...state,
        selectedTypes: state.selectedTypes.includes(action.type)
          ? state.selectedTypes
          : [...state.selectedTypes, action.type],
        typeSessions: {
          ...state.typeSessions,
          [action.type]: {
            ...newTypeSession(action.type),
            revisions: [revision],
            activeRevisionIndex: 0,
            editingExistingNamespace: action.namespace,
          },
        },
        currentStep: "typeDetail",
        activeType: action.type,
      }
    }
    default:
      return state
  }
}

/** T004's in-flight-operation guard (spec Edge Case: "user triggers a new generate/fix/edit
 * action while a trial run is still in progress"): tracks which types currently have a
 * generate/trial-run/save call in flight, so the UI can disable the actions that would start a
 * conflicting one for that same type. A different type is never blocked by another type's
 * in-flight operation. */
function useInFlightGuard() {
  const [inFlight, setInFlight] = useReducer(
    (state: Partial<Record<PluginType, boolean>>, patch: Partial<Record<PluginType, boolean>>) => ({
      ...state,
      ...patch,
    }),
    {},
  )

  const isInFlight = useCallback((type: PluginType) => Boolean(inFlight[type]), [inFlight])
  const beginOperation = useCallback((type: PluginType) => setInFlight({ [type]: true }), [])
  const endOperation = useCallback((type: PluginType) => setInFlight({ [type]: false }), [])

  return { isInFlight, beginOperation, endOperation }
}

/** FR-013: whether `type` is currently allowed to start a `generate` call. A metadata/download
 * type marked `dependsOnLogin === true` must wait until the `login` `TypeSession` has a revision
 * with at least one successful trial run — types that don't depend on login (or the login type
 * itself) are never subject to this ordering. */
export function canGenerateFor(session: WizardSession, type: PluginType): boolean {
  const typeSession = session.typeSessions[type]
  if (!typeSession || typeSession.dependsOnLogin !== true) return true

  // An `"existing"` association means the login plugin is already a real, installed plugin (found
  // covered at the initial lookup step) — there's nothing to wait on a trial run *for*, it's
  // already usable (and `generate.rs`'s own credential resolution reads its real Redis-persisted
  // values directly). The trial-run-gate below only makes sense for a login plugin *this session*
  // is also generating, where "hasn't been verified to actually work yet" is a real risk. Without
  // this branch, answering "depends on login: yes" for an already-installed login plugin
  // permanently disabled Generate — `session.typeSessions.login` never exists in that case at all,
  // since there's no login `TypeSession` to select/generate (a real reported gap, 2026-08-25).
  if (typeSession.loginAssociation?.source === "existing") return true

  const loginSession = session.typeSessions.login
  if (!loginSession) return false
  return loginSession.revisions.some((revision) =>
    revision.trialRuns.some((run) => run.type === "login" && run.outcome === "success"),
  )
}

/** US3 AC6 (T028): a type's *active* revision can only be confirm-saved once it has a trial-run
 * result for every required target — every one of `WizardSession.sharedLinks` for metadata/
 * download (re-checked by count, not just "at least one" — adding a link after a trial run must
 * re-gate), or the one login attempt for `login`. */
export function canSaveFor(session: WizardSession, typeSession: TypeSession): boolean {
  const activeRevision = typeSession.revisions[typeSession.activeRevisionIndex]
  if (!activeRevision) return false

  if (typeSession.type === "login") {
    return activeRevision.trialRuns.some((r) => r.type === "login")
  }

  const triedLinks = new Set(
    activeRevision.trialRuns.flatMap((r) => (r.type === "login" ? [] : r.perLink.map((l) => l.link))),
  )
  const links = cleanLinks(session.sharedLinks)
  return links.length > 0 && links.every((link) => triedLinks.has(link))
}

/** Whether the wizard's shared input (`WizardSession.sharedLinks`, or credentials for `login`) is
 * complete enough to allow triggering generation for `typeSession` — mirrors the backend's own
 * minimum requirements (FR-005: ≥3 links for metadata/download, all required credential fields
 * for login). Drives the four-substep wizard layout in `index.tsx`: form → generate → trial-run →
 * save, each substep only shown once the previous one is satisfied. */
export function isFormComplete(session: WizardSession, typeSession: TypeSession): boolean {
  if (typeSession.type === "login") {
    if (!typeSession.loginParameters) return false
    return typeSession.loginParameters
      .filter((p) => p.required)
      .every((p) => typeSession.loginFieldValues[p.name]?.trim())
  }
  return cleanLinks(session.sharedLinks).length >= 3
}

/** Gates the shared-links step's "下一步" button — only metadata/download selected types actually
 * need `sharedLinks` (≥3, FR-005); a selected `login` type is gated by its own separate
 * analyze-login form instead (`TypeDetailsForm.tsx`), which the shared-links step never blocks on.
 * `true` when every metadata/download selected type's link requirement is already met, or when
 * none of the selected types need links at all (login-only selection). */
export function isSharedLinksStepComplete(session: WizardSession): boolean {
  const linkDependentTypes = session.selectedTypes.filter((t) => t !== "login")
  if (linkDependentTypes.length === 0) return true
  return cleanLinks(session.sharedLinks).length >= 3
}

/** FR-018: the fixed cap on *consecutive* AI auto-fix attempts for one type. */
export const AUTO_FIX_ATTEMPT_CAP = 3

export function canAutoFixFor(typeSession: TypeSession): boolean {
  return typeSession.autoFixAttemptsUsed < AUTO_FIX_ATTEMPT_CAP
}

/** Summarizes the active revision's most recent trial-run failure(s) into the `previous_error`
 * text an AI auto-fix (US5/FR-017) sends back to `/plugin-wizard/generate` — `undefined` when
 * there's nothing to fix (no trial run yet, or the most recent one fully succeeded). */
export function latestFailureSummary(typeSession: TypeSession): string | undefined {
  const activeRevision = typeSession.revisions[typeSession.activeRevisionIndex]
  const lastRun = activeRevision?.trialRuns[activeRevision.trialRuns.length - 1]
  if (!lastRun) return undefined

  if (lastRun.type === "login") {
    return lastRun.outcome === "failure" ? lastRun.detail : undefined
  }
  const failures = lastRun.perLink.filter((l) => l.outcome === "failure")
  if (failures.length === 0) return undefined
  return failures.map((l) => `${l.link}: ${l.error ?? "unknown error"}`).join("\n")
}

export function useWizardSession() {
  const [session, dispatch] = useReducer(reducer, null)
  const inFlightGuard = useInFlightGuard()

  return {
    session,
    dispatch,
    ...inFlightGuard,
  }
}
