import { useCallback, useReducer } from "react"

/** Every entity here is frontend-only state; nothing in this file is ever persisted server-side. */

export type PluginType = "login" | "metadata" | "download"

/** `WizardSession.sharedLinks` stores raw, unfiltered textarea lines; every consumer calls this
 * once at the point of use rather than filtering the stored list itself. */
export function cleanLinks(links: string[]): string[] {
  return links.map((l) => l.trim()).filter(Boolean)
}

/** Login credential values, only available when the login plugin was generated this session. */
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

/** What kind of installed plugin a coverage hit points at. */
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

/** The association's file-path namespace — distinct from the declared `namespace`. */
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

/** One credential field — not assumed to be a username/password pair. */
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
  origin: "ai-generated" | "ai-auto-fix" | "manual-edit" | "ai-refine" | "loaded-from-existing"
  createdAt: number
  trialRuns: TrialRunResult[]
  /** AI explanation; absent (not empty) for non-`ai-*` origins. */
  explanation?: string
}

/** One past round of this type's generation conversation. */
export interface ConversationTurn {
  userMessage: string
  assistantCode: string
}

export interface TypeSession {
  type: PluginType
  /** Login only; `null` until analysis succeeds. */
  loginParameters: LoginParameter[] | null
  /** Login only — never sent to `generate`/AI-auto-fix payloads, only to `trial-run`. */
  loginFieldValues: Record<string, string>
  /** Login only — the one URL `analyze-login` inspected. Sending all of `sharedLinks` instead made
   * the AI fetch every sample link too and blow the generate timeout. Stored, not recomputed. */
  loginReferenceUrl: string
  /** Metadata/download only; `null` until explicitly answered — never defaulted by the UI. */
  dependsOnLogin: boolean | null
  loginAssociation: LoginAssociation | null
  /** Every past (asked, code) pair, replayed as `conversation_history` on each generate call. */
  conversationHistory: ConversationTurn[]
  revisions: DraftRevision[]
  activeRevisionIndex: number
  /** Consecutive auto-fix count — a manual edit or fresh generation resets it. */
  autoFixAttemptsUsed: number
  /** The real installed file-path namespace once saved; `null` while still a draft. */
  savedNamespace: string | null
  /** The plugin's declared namespace — what another type's `loginAssociation.namespace` matches. */
  savedDeclaredNamespace: string | null
  /** Login values resolved server-side, used to auto-prefill the trial-run parameter panel. */
  resolvedCredentialValues: Record<string, string>
  /** File-path namespace this edit-mode session must write back to on save. */
  editingExistingNamespace: string | null
}

export type WizardStep = "typeSelection" | "sharedLinks" | "typeDetail"

export interface WizardSession {
  domain: string
  lookupResult: DomainLookupResult
  selectedTypes: PluginType[]
  typeSessions: Partial<Record<PluginType, TypeSession>>
  /** No `"lookup"` member here — that step is `session === null` in `index.tsx`. */
  currentStep: WizardStep
  /** The one panel mounted while on `"typeDetail"`; `null` otherwise. */
  activeType: PluginType | null
  /** Domain-level shared links (one per line) — the single input the whole wizard works from. */
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
      // Deselecting discards the TypeSession entirely; reselecting starts fresh, never resurrects.
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
      // Step-indicator navigation; forward clicks are ignored — only "下一步" advances.
      kind: "goToStep"
      step: WizardStep
    }
  | {
      // Jumps straight into edit mode, seeded from the plugin's already-saved source.
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
      // The cap is on consecutive auto-fix attempts — anything else resets the chain.
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

/** Tracks which types have an in-flight generate/trial-run/save, per type — never cross-type. */
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

/** Whether `type` may start a `generate` call — a `dependsOnLogin` type waits on a successful
 * login trial run. */
export function canGenerateFor(session: WizardSession, type: PluginType): boolean {
  const typeSession = session.typeSessions[type]
  if (!typeSession || typeSession.dependsOnLogin !== true) return true

  // An already-installed login plugin has nothing to wait on — no login TypeSession exists then,
  // and without this branch Generate would be permanently disabled.
  if (typeSession.loginAssociation?.source === "existing") return true

  const loginSession = session.typeSessions.login
  if (!loginSession) return false
  return loginSession.revisions.some((revision) =>
    revision.trialRuns.some((run) => run.type === "login" && run.outcome === "success"),
  )
}

/** The active revision is saveable once every shared link (or the one login attempt) has a
 * trial-run result — re-checked by count, so adding a link re-gates. */
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

/** Whether the shared input is complete enough to generate: ≥3 links for metadata/download, all
 * required credential fields for login. */
export function isFormComplete(session: WizardSession, typeSession: TypeSession): boolean {
  if (typeSession.type === "login") {
    if (!typeSession.loginParameters) return false
    return typeSession.loginParameters
      .filter((p) => p.required)
      .every((p) => typeSession.loginFieldValues[p.name]?.trim())
  }
  return cleanLinks(session.sharedLinks).length >= 3
}

/** Gates the shared-links step's "下一步" — login-only selections never need links. */
export function isSharedLinksStepComplete(session: WizardSession): boolean {
  const linkDependentTypes = session.selectedTypes.filter((t) => t !== "login")
  if (linkDependentTypes.length === 0) return true
  return cleanLinks(session.sharedLinks).length >= 3
}

/** Cap on consecutive AI auto-fix attempts for one type. */
export const AUTO_FIX_ATTEMPT_CAP = 3

export function canAutoFixFor(typeSession: TypeSession): boolean {
  return typeSession.autoFixAttemptsUsed < AUTO_FIX_ATTEMPT_CAP
}

/** The `previous_error` text for an auto-fix; `undefined` when nothing failed. */
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
