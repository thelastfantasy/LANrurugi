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

/** Default save filename: domain's alphanumeric characters only, matching
 * `save.rs::is_safe_filename`'s allowed charset. */
function defaultFilename(domain: string): string {
  return domain.replace(/[^a-zA-Z0-9_-]/g, "_")
}

/** The locked (not user-editable) filename stem an edit-mode session must save back to — the
 * last path segment of the namespace it was loaded from. */
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
  /** Only populates after the first trial run, once the server-side `plugin_info()` probe
   * discovers the draft's declared parameters. */
  declared_parameters: DeclaredParameter[]
  /** Download drafts only; `undefined` when the draft declares no `pluginOptions()` at all. */
  declared_options?: PluginOptions
}

interface LoginTrialRunResponse {
  outcome: "success" | "failure"
  detail: string
}

/** Triggers `POST /plugin-wizard/trial-run` for the type's active revision and renders every
 * past trial-run result for that revision independently. */
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
  /** Fired after a successful save, carrying the installed namespace and the plugin's own
   * self-declared `pluginInfo().namespace` (what a different plugin's `login_from` references). */
  onSaved: (namespace: string, declaredNamespace: string) => void
  onRevisionAppended: (revision: DraftRevision) => void
  onConversationTurnAppended: (turn: ConversationTurn) => void
  onCredentialValuesResolved: (values: Record<string, string>) => void
}) {
  const { t } = useTranslation()
  const isEditingExisting = typeSession.editingExistingNamespace !== null
  const [filename, setFilename] = useState(() =>
    typeSession.editingExistingNamespace
      ? existingFilenameStem(typeSession.editingExistingNamespace)
      : defaultFilename(domain),
  )
  const [autoFixing, setAutoFixing] = useState(false)
  const { items, elapsedSeconds, start: startProgress, stop: stopProgress, onProgress } = useGenerationProgress()
  // `undefined` means "never trial-run yet", not "declares no parameters". Reset (during render,
  // not via useEffect) whenever the active revision changes so stale values don't carry over.
  const [declaredParameters, setDeclaredParameters] = useState<DeclaredParameter[] | undefined>(undefined)
  const [pluginParameterValues, setPluginParameterValues] = useState<Record<string, string>>({})
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
        // A login plugin generated this session was never persisted to Redis, so supply its
        // field values directly rather than relying on trial_run.rs's normal Redis lookup.
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
      const pluginParameterValuesToPersist = type === "login" ? typeSession.loginFieldValues : pluginParameterValues
      const response = await sendJson<{ namespace: string; declared_namespace: string }>(
        "POST",
        "/plugin-wizard/save",
        {
          plugin_type: type,
          code: activeRevision.code,
          filename,
          plugin_parameter_values: pluginParameterValuesToPersist,
          // `save.rs` rejects `allow_overwrite: true` unless `overwrite_namespace` matches exactly.
          ...(isEditingExisting
            ? { allow_overwrite: true, overwrite_namespace: typeSession.editingExistingNamespace }
            : {}),
        },
      )
      toast({ text: t("pluginWizard.saveSucceeded", { namespace: response.namespace }) ?? undefined, icon: "success" })
      toast({ text: t("pluginWizard.exportReminder") ?? undefined, icon: "info" })
      onSaved(response.namespace, response.declared_namespace)
    } catch (err) {
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
