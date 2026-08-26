import { useState } from "react"
import { useTranslation } from "react-i18next"

import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { toast } from "@/toast"

import { GenerateStreamError, streamGenerate } from "./generateStream"
import { GenerationProgressView } from "./GenerationProgressView"
import { useGenerationProgress } from "./useGenerationProgress"
import type { ConversationTurn, DraftRevision, PluginType, TypeSession, WizardSession } from "./useWizardSession"
import { availableCredentialsFor, canGenerateFor, cleanLinks, isFormComplete } from "./useWizardSession"

/** FR-008/FR-013: triggers the streaming `/plugin-wizard/generate/start`+`/stream/{id}` pair
 * (`generateStream.ts`), gated by `canGenerateFor` (the login-dependency ordering rule), and
 * appends the result as a new `DraftRevision`. Also the entry point US5's "AI auto-fix" reuses
 * (T036) via `previousCode`/`previousError`. */
export function GenerationStep({
  session,
  typeSession,
  type,
  isInFlight,
  onBeginOperation,
  onEndOperation,
  onRevisionAppended,
  onConversationTurnAppended,
  onCredentialValuesResolved,
  previousCode,
  previousError,
  label,
}: {
  session: WizardSession
  typeSession: TypeSession
  type: PluginType
  isInFlight: boolean
  onBeginOperation: () => void
  onEndOperation: () => void
  onRevisionAppended: (revision: DraftRevision) => void
  onConversationTurnAppended: (turn: ConversationTurn) => void
  /** See `TypeSession.resolvedCredentialValues`'s own docs — fired whenever this call actually had
   * a login association to resolve, even if the resulting map is empty (an empty map still needs
   * to overwrite a stale non-empty one from a prior association, e.g. after the user switches which
   * login plugin this type depends on). */
  onCredentialValuesResolved: (values: Record<string, string>) => void
  /** Present only for an AI-auto-fix round (US5) — `undefined` for a fresh generation. */
  previousCode?: string
  previousError?: string
  label?: string
}) {
  const { t } = useTranslation()
  const canGenerate = canGenerateFor(session, type) && isFormComplete(session, typeSession)
  const origin = previousCode !== undefined ? "ai-auto-fix" : "ai-generated"
  // `isInFlight` (prop) is shared across generate/trial-run/save for this type (T004's per-type
  // guard, not per-action) — using it directly to gate the skeleton would show "AI is thinking"
  // under the Generate button while a trial-run or save is running instead, which never involves
  // AI. This local flag tracks only *this* component's own in-flight generate call.
  const [generating, setGenerating] = useState(false)
  const { items, elapsedSeconds, start, stop, onProgress } = useGenerationProgress()

  async function trigger() {
    onBeginOperation()
    setGenerating(true)
    start()
    try {
      const loginAssociation = typeSession.loginAssociation
        ? { namespace: typeSession.loginAssociation.namespace }
        : undefined
      // Login only needs its own reference URL inspected, not every metadata/download sample link
      // pasted for the other selected types — see `TypeSession.loginReferenceUrl`'s own doc
      // comment for why sending the full shared list here caused a real generate-timeout.
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
          reference_sample_code: session.lookupResult[type].covered
            ? undefined
            : findSameDomainSample(session, type),
          login_association: loginAssociation,
          login_parameters: type === "login" ? typeSession.loginParameters : undefined,
          available_credential_fields: availableCredentialFields,
          credential_values: credentialValues,
          previous_code: previousCode ?? null,
          previous_error: previousError ?? null,
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
        origin,
        createdAt: Date.now(),
        trialRuns: [],
        explanation: response.explanation,
      })
      onConversationTurnAppended({
        userMessage: previousError ? `上一版代码试运行失败，请修复：${previousError}` : "请生成插件代码。",
        assistantCode: response.code,
      })
    } catch (err) {
      const heading =
        err instanceof GenerateStreamError && err.code === "ai_output_not_code"
          ? t("pluginWizard.aiOutputNotCode")
          : t("pluginWizard.generateFailed")
      toast({ heading: heading ?? undefined, text: String(err), icon: "error" })
    } finally {
      setGenerating(false)
      stop()
      onEndOperation()
    }
  }

  return (
    <div>
      <input
        type="button"
        className="stdbtn"
        value={label ?? (t("pluginWizard.generate") ?? "")}
        disabled={!canGenerate || isInFlight}
        title={!canGenerate ? (t("pluginWizard.waitingOnLoginPlugin") ?? undefined) : undefined}
        onClick={() => void trigger()}
      />
      {/* Same "AI is thinking" treatment as TankoubonEdit.tsx/Categories.tsx's own AI calls. */}
      {generating && <AiSkeleton />}
      <GenerationProgressView items={items} elapsedSeconds={elapsedSeconds} />
    </div>
  )
}

/** FR-009: the same-domain reference sample the lookup step already fetched for a *different*
 * type — e.g. generating "metadata" for a domain whose "download" plugin is already installed
 * reuses that download plugin's own source as a structural reference. Only relevant when this
 * type itself isn't the covered one (the lookup result already carries this type's own coverage
 * separately, handled by the caller). */
function findSameDomainSample(session: WizardSession, type: PluginType): string | undefined {
  const others: PluginType[] = (["login", "metadata", "download"] as PluginType[]).filter((t) => t !== type)
  for (const other of others) {
    const coverage = session.lookupResult[other]
    if (coverage.covered) return coverage.sourceCode
  }
  return undefined
}
