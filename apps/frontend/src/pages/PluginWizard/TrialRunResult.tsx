import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/apiError"
import { sendJson } from "@/api/client"
import type { PluginOptions } from "@/api/types"
import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { toast } from "@/toast"

import { AssociatedLoginPluginSettings } from "./AssociatedLoginPluginSettings"
import { GenerateStreamError, streamGenerate } from "./generateStream"
import { GenerationProgressView } from "./GenerationProgressView"
import { useGenerationProgress } from "./useGenerationProgress"
import type {
  ConversationTurn,
  DraftRevision,
  PluginType,
  TrialRunResult as TrialRunResultData,
  TypeSession,
  WizardSession,
} from "./useWizardSession"
import {
  availableCredentialsFor,
  canAutoFixFor,
  canSaveFor,
  cleanLinks,
  latestFailureSummary,
  loginPluginFilePathNamespace,
} from "./useWizardSession"

/** Default save filename: the domain's own alphanumeric characters only (`example.com` →
 * `example_com`), matching `save.rs::is_safe_filename`'s allowed charset — always editable by the
 * user before confirming (FR-021's rename-and-retry needs a real input to retry into). */
function defaultFilename(domain: string): string {
  return domain.replace(/[^a-zA-Z0-9_-]/g, "_")
}

/** The file stem an edit-mode session (`editingExistingNamespace`) must save back to — the last
 * path segment of the file-path namespace it was loaded from (`custom/metadata/foosite` →
 * `"foosite"`), locked (not user-editable) so a re-save can only ever overwrite the same file it
 * came from, never accidentally rename into a fresh, unrelated one. */
function existingFilenameStem(namespace: string): string {
  return namespace.split("/").pop() ?? namespace
}

interface PerLinkResponse {
  link: string
  outcome: "success" | "failure"
  data?: unknown
  error?: string
}

interface DeclaredParameter {
  name: string
  description: string
  required: boolean
}

interface LinkTrialRunResponse {
  per_link: PerLinkResponse[]
  login_suggestion?: { relevant: boolean; reasoning: string }
  /** The draft's own declared `pluginInfo().parameters` — the wizard has no way to know these
   * ahead of a real trial run (the server-side `plugin_info()` probe is what actually discovers
   * them), so this only ever populates *after* the first trial run, at which point the rendered
   * inputs below let the user supply real values and retry. */
  declared_parameters: DeclaredParameter[]
  /** Download drafts only — the same effective-settings shape `GET /plugins/options` returns for
   * an installed plugin (`PluginOptions` in `api/types.ts`), probed fresh from the staged draft's
   * own `pluginOptions()` export on every trial run — `undefined` when this draft declares none at
   * all (a metadata draft, or a download draft with no rate-limit/bundling opinion). Rendering this
   * live off the trial-run response (rather than assuming any fixed shape) is what makes the panel
   * reflect the draft's *actual current code* — see this session's own "根据生成的或已存在的插件
   * 代码实时渲染" requirement. */
  declared_options?: PluginOptions
}

interface LoginTrialRunResponse {
  outcome: "success" | "failure"
  detail: string
}

/** T027 (US3) — triggers `POST /plugin-wizard/trial-run` for the type's *active* revision and
 * renders every past trial-run result for that revision independently (AC4: no single verdict
 * masking individual link outcomes). */
export function TrialRunResult({
  session,
  domain,
  typeSession,
  type,
  activeRevision,
  isInFlight,
  onBeginOperation,
  onEndOperation,
  onTrialRunAppended,
  onSaved,
  onRevisionAppended,
  onConversationTurnAppended,
  onCredentialValuesResolved,
}: {
  session: WizardSession
  domain: string
  typeSession: TypeSession
  type: PluginType
  activeRevision: DraftRevision | undefined
  isInFlight: boolean
  onBeginOperation: () => void
  onEndOperation: () => void
  onTrialRunAppended: (result: TrialRunResultData) => void
  /** T031 — fired after a successful `/plugin-wizard/save`, carrying the newly installed
   * namespace and the plugin's own self-declared `pluginInfo().namespace` (the value a *different*
   * plugin's `login_from` must reference — see save.rs's own doc comment on `declared_namespace`),
   * so the caller can mark this type as saved/finalized and, for a login type, make its real
   * declared namespace available for T043's "link to this login plugin" action. */
  onSaved: (namespace: string, declaredNamespace: string) => void
  /** T036 (US5) — fired once an AI auto-fix call returns a new draft. */
  onRevisionAppended: (revision: DraftRevision) => void
  onConversationTurnAppended: (turn: ConversationTurn) => void
  /** See `TypeSession.resolvedCredentialValues`'s own docs. */
  onCredentialValuesResolved: (values: Record<string, string>) => void
}) {
  const { t } = useTranslation()
  const isEditingExisting = typeSession.editingExistingNamespace !== null
  const [filename, setFilename] = useState(() =>
    typeSession.editingExistingNamespace
      ? existingFilenameStem(typeSession.editingExistingNamespace)
      : defaultFilename(domain),
  )
  // `isInFlight` (prop) is shared across trial-run/save/AI-auto-fix for this type (T004's
  // per-type guard) — gating the AI skeleton on it directly would show "AI is thinking" during a
  // plain trial-run or save too. This local flag tracks only the auto-fix call's own AI round-trip.
  const [autoFixing, setAutoFixing] = useState(false)
  const { items, elapsedSeconds, start: startProgress, stop: stopProgress, onProgress } = useGenerationProgress()
  // The draft's own declared `pluginInfo().parameters` (e.g. a generated download plugin's own
  // `api_key`) and the user's values for them — only known once a trial run's own server-side
  // `plugin_info()` probe reports them back (see `LinkTrialRunResponse.declared_parameters`'s own
  // docs); `undefined` values means "never trial-run yet", not "declares no parameters". Reset
  // whenever the active revision changes — a different draft's code may declare different (or no)
  // parameters, so stale inputs from a previous revision must not carry over. Adjusted during
  // render (React's own documented pattern for "reset state when a prop changes"), not via a
  // `useEffect`, which would need an extra render pass and trips `react-hooks/set-state-in-effect`.
  const [declaredParameters, setDeclaredParameters] = useState<DeclaredParameter[] | undefined>(undefined)
  const [pluginParameterValues, setPluginParameterValues] = useState<Record<string, string>>({})
  // The draft's own `pluginOptions()` (domain rate-limit rules / bundle-as-archive), same "only
  // known after a real trial-run probe" reasoning as `declaredParameters` above — reset alongside
  // it for the same reason: a different revision's code may declare an entirely different (or no)
  // `pluginOptions()`, so a stale panel from a previous revision must not linger.
  const [declaredOptions, setDeclaredOptions] = useState<PluginOptions | undefined>(undefined)
  const [lastRevisionId, setLastRevisionId] = useState(activeRevision?.id)
  if (activeRevision?.id !== lastRevisionId) {
    setLastRevisionId(activeRevision?.id)
    setDeclaredParameters(undefined)
    setPluginParameterValues({})
    setDeclaredOptions(undefined)
  }

  async function trigger() {
    if (!activeRevision) return
    onBeginOperation()
    try {
      if (type === "login") {
        const response = await sendJson<LoginTrialRunResponse>("POST", "/plugin-wizard/trial-run", {
          plugin_type: type,
          code: activeRevision.code,
          credentials: { fields: typeSession.loginFieldValues },
        })
        onTrialRunAppended({ type: "login", outcome: response.outcome, detail: response.detail })
      } else {
        // FR-025: if this draft is associated with a login plugin generated *this session*, the
        // login field values the user typed into that login TypeSession were never persisted to
        // Redis (FR-012) — supply them directly so trial_run.rs can run a fresh login call
        // instead of finding nothing via its normal Redis-persisted-settings lookup.
        const loginFieldValues =
          typeSession.loginAssociation?.source === "generated-this-session"
            ? session.typeSessions.login?.loginFieldValues
            : undefined
        const loginCredentials = loginFieldValues ? { fields: loginFieldValues } : undefined
        const response = await sendJson<LinkTrialRunResponse>("POST", "/plugin-wizard/trial-run", {
          plugin_type: type,
          code: activeRevision.code,
          test_links: cleanLinks(session.sharedLinks),
          login_credentials: loginCredentials ?? undefined,
          plugin_parameter_values: pluginParameterValues,
        })
        setDeclaredParameters(response.declared_parameters)
        setDeclaredOptions(response.declared_options)
        // Auto-prefill by field-name match against whatever the backend already resolved from the
        // associated login plugin's own persisted Redis settings (real report, 2026-08-25: "这里
        // 还是没读取到已经设置的key") — only fills fields the user hasn't already typed something
        // into, so a value entered directly here always wins over a stale/different resolved one.
        if (Object.keys(typeSession.resolvedCredentialValues).length > 0) {
          setPluginParameterValues((prev) => {
            const next = { ...prev }
            for (const param of response.declared_parameters) {
              if (next[param.name]) continue
              const resolved = typeSession.resolvedCredentialValues[param.name]
              if (resolved) next[param.name] = resolved
            }
            return next
          })
        }
        onTrialRunAppended({
          type,
          perLink: response.per_link.map((r) => ({
            link: r.link,
            outcome: r.outcome,
            data: r.data,
            error: r.error,
          })),
          loginSuggestion: response.login_suggestion,
        })
      }
    } catch (err) {
      toast({ heading: t("pluginWizard.trialRunFailed") ?? undefined, text: String(err), icon: "error" })
    } finally {
      onEndOperation()
    }
  }

  async function triggerSave() {
    if (!activeRevision) return
    onBeginOperation()
    try {
      // Whatever real values the user already typed in this wizard session — the type's own
      // login credential fields for a login draft, or the trial-run parameter inputs above for a
      // metadata/download draft — persisted straight into the newly-installed plugin's real
      // settings on save, so it's immediately ready to use instead of needing a second manual trip
      // to the plugin settings page to re-enter the same values (see `save.rs`'s own docs).
      const pluginParameterValuesToPersist = type === "login" ? typeSession.loginFieldValues : pluginParameterValues
      const response = await sendJson<{ namespace: string; declared_namespace: string }>(
        "POST",
        "/plugin-wizard/save",
        {
          plugin_type: type,
          code: activeRevision.code,
          filename,
          plugin_parameter_values: pluginParameterValuesToPersist,
          // Only meaningful together — `save.rs` rejects `allow_overwrite: true` unless
          // `overwrite_namespace` exactly matches the namespace this request itself resolves to,
          // so a stale/mismatched session state can't accidentally overwrite the wrong file.
          ...(isEditingExisting
            ? { allow_overwrite: true, overwrite_namespace: typeSession.editingExistingNamespace }
            : {}),
        },
      )
      toast({ text: t("pluginWizard.saveSucceeded", { namespace: response.namespace }) ?? undefined, icon: "success" })
      // FR-031: the host process can't tell whether plugins_dir is host-mounted or ephemeral, so
      // this reminder always shows on a successful save, not only when persistence is actually at
      // risk — see plugins.rs's own export endpoint this points at.
      toast({ text: t("pluginWizard.exportReminder") ?? undefined, icon: "info" })
      onSaved(response.namespace, response.declared_namespace)
    } catch (err) {
      // `error_typed`'s server-side shape (`{"error": kind, "detail": ...}`) is folded into
      // `ApiError.message` as `"kind: detail"` by `readErrorBody` — no separate structured field
      // to check, so this matches the same way `TypeDetailsForm.tsx`'s own 422 handling does.
      const heading =
        err instanceof ApiError && err.status === 400 && err.message.startsWith("overwrite_namespace_mismatch")
          ? t("pluginWizard.overwriteConflict")
          : err instanceof ApiError && err.status === 409
            ? t("pluginWizard.saveConflict")
            : t("pluginWizard.saveFailed")
      toast({ heading: heading ?? undefined, text: String(err), icon: "error" })
    } finally {
      onEndOperation()
    }
  }

  async function triggerAutoFix() {
    if (!activeRevision) return
    const previousError = latestFailureSummary(typeSession)
    if (!previousError) return
    onBeginOperation()
    setAutoFixing(true)
    startProgress()
    try {
      const loginAssociation = typeSession.loginAssociation
        ? { namespace: typeSession.loginAssociation.namespace }
        : undefined
      // See GenerationStep.tsx's own identical narrowing — same reasoning applies to auto-fix.
      const links = type === "login" ? [typeSession.loginReferenceUrl].filter(Boolean) : cleanLinks(session.sharedLinks)
      const { fields: availableCredentialFields, values: credentialValues } = availableCredentialsFor(
        session,
        typeSession,
      )
      const response = await streamGenerate(
        {
          plugin_type: type,
          test_links: links,
          auxiliary_reference_urls: links,
          login_association: loginAssociation,
          login_parameters: type === "login" ? typeSession.loginParameters : undefined,
          available_credential_fields: availableCredentialFields,
          credential_values: credentialValues,
          previous_code: activeRevision.code,
          previous_error: previousError,
          conversation_history: typeSession.conversationHistory.map((turn) => ({
            user_message: turn.userMessage,
            assistant_code: turn.assistantCode,
          })),
        },
        onProgress,
      )
      if (loginAssociation) onCredentialValuesResolved(response.resolvedCredentialValues)
      onRevisionAppended({
        id: crypto.randomUUID(),
        code: response.code,
        origin: "ai-auto-fix",
        createdAt: Date.now(),
        trialRuns: [],
        explanation: response.explanation,
      })
      onConversationTurnAppended({
        userMessage: `上一版代码试运行失败，请修复：${previousError}`,
        assistantCode: response.code,
      })
    } catch (err) {
      const heading =
        err instanceof GenerateStreamError && err.code === "ai_output_not_code"
          ? t("pluginWizard.aiOutputNotCode")
          : t("pluginWizard.generateFailed")
      toast({ heading: heading ?? undefined, text: String(err), icon: "error" })
    } finally {
      setAutoFixing(false)
      stopProgress()
      onEndOperation()
    }
  }

  const canSave = canSaveFor(session, typeSession)
  const failureSummary = latestFailureSummary(typeSession)
  const canAutoFix = Boolean(failureSummary) && canAutoFixFor(typeSession)

  if (typeSession.savedNamespace) {
    return <p className="ptbox" style={{ marginTop: 8, padding: 8 }}>{t("pluginWizard.savedAs", { namespace: typeSession.savedNamespace })}</p>
  }

  const loginFilePathNamespace = loginPluginFilePathNamespace(session, typeSession.loginAssociation)

  return (
    <div style={{ marginTop: 8 }}>
      {type !== "login" && typeSession.dependsOnLogin && (
        <AssociatedLoginPluginSettings filePathNamespace={loginFilePathNamespace} />
      )}
      {type !== "login" && declaredOptions && (
        <div className="ptbox" style={{ padding: 8, marginBottom: 8 }}>
          <h3 className="ih">{t("pluginWizard.declaredOptionsPreviewHeading")}</h3>
          <p style={{ fontStyle: "italic", margin: "0 0 6px" }}>
            {t("pluginWizard.declaredOptionsPreviewHint")}
          </p>
          {declaredOptions.domain_rules.length > 0 && (
            <table className="itg" style={{ width: "100%" }}>
              <thead>
                <tr className="jtr0">
                  <th>{t("pluginOptions.domainPattern")}</th>
                  <th>{t("pluginOptions.maxConcurrent")}</th>
                  <th>{t("pluginOptions.maxKbS")}</th>
                </tr>
              </thead>
              <tbody>
                {declaredOptions.domain_rules.map((rule, i) => (
                  <tr key={i} className="gtr1">
                    <td>{rule.pattern}</td>
                    <td>{rule.max_concurrent ?? "-"}</td>
                    <td>{rule.max_bytes_per_sec != null ? Math.round(rule.max_bytes_per_sec / 1024) : "-"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {declaredOptions.bundle_as_archive && <p>{t(declaredOptions.bundle_as_archive.description)}</p>}
          {declaredOptions.overwrite_on_duplicate && <p>{t(declaredOptions.overwrite_on_duplicate.description)}</p>}
        </div>
      )}
      {type !== "login" && declaredParameters && declaredParameters.length > 0 && (
        <div className="ptbox" style={{ padding: 8, marginBottom: 8 }}>
          <h3 className="ih">{t("pluginWizard.declaredParametersHeading")}</h3>
          {declaredParameters.map((param) => (
            <label key={param.name} style={{ display: "block", marginTop: 4 }}>
              {param.description}
              {param.required && " *"}
              <input
                className="stdinput"
                type={/secret|password|token|key|cookie/i.test(param.name) ? "password" : "text"}
                value={pluginParameterValues[param.name] ?? ""}
                placeholder={param.description}
                onChange={(e) =>
                  setPluginParameterValues((prev) => ({ ...prev, [param.name]: e.target.value }))
                }
                style={{ width: "100%", display: "block" }}
              />
            </label>
          ))}
        </div>
      )}
      <input
        type="button"
        className="stdbtn"
        value={t("pluginWizard.trialRun") ?? ""}
        disabled={!activeRevision || isInFlight}
        onClick={() => void trigger()}
      />
      {failureSummary && (
        <>
          <input
            type="button"
            className="stdbtn"
            style={{ marginLeft: 8 }}
            value={t("pluginWizard.aiAutoFix") ?? ""}
            disabled={!canAutoFix || isInFlight}
            title={!canAutoFix ? (t("pluginWizard.autoFixCapReached") ?? undefined) : undefined}
            onClick={() => void triggerAutoFix()}
          />
          {autoFixing && <AiSkeleton />}
          <GenerationProgressView items={items} elapsedSeconds={elapsedSeconds} />
        </>
      )}
      <label style={{ marginLeft: 8 }}>
        {t("pluginWizard.saveFilenameLabel")}
        <input
          className="stdinput"
          type="text"
          value={filename}
          onChange={(e) => setFilename(e.target.value)}
          disabled={isEditingExisting}
          title={isEditingExisting ? (t("pluginWizard.overwriteFilenameLocked") ?? undefined) : undefined}
          style={{ marginLeft: 4, width: 160 }}
        />
      </label>
      <input
        type="button"
        className="stdbtn"
        style={{ marginLeft: 8 }}
        value={t(isEditingExisting ? "pluginWizard.confirmOverwrite" : "pluginWizard.confirmSave") ?? ""}
        disabled={!canSave || isInFlight || !filename}
        title={!canSave ? (t("pluginWizard.saveGateHint") ?? undefined) : undefined}
        onClick={() => void triggerSave()}
      />

      {activeRevision?.trialRuns.map((result, i) => (
        <div key={i} className="ptbox" style={{ marginTop: 8, padding: 8 }}>
          {result.type === "login" ? (
            <p>
              <strong>{t(`pluginWizard.outcome.${result.outcome}`)}</strong>: {result.detail}
            </p>
          ) : (
            <>
              {result.perLink.map((link, j) => (
                <p key={j}>
                  <strong>{t(`pluginWizard.outcome.${link.outcome}`)}</strong> — {link.link}
                  {link.error ? `: ${link.error}` : null}
                </p>
              ))}
              {result.loginSuggestion?.relevant && (
                <p style={{ fontStyle: "italic" }}>
                  {t("pluginWizard.loginSuggestionHint")}: {result.loginSuggestion.reasoning}
                </p>
              )}
            </>
          )}
        </div>
      ))}
    </div>
  )
}
