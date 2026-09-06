import { useState } from "react"
import { useTranslation } from "react-i18next"

import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { toast } from "@/toast"

import { GenerateStreamError, streamGenerate } from "./generateStream"
import { GenerationProgressView } from "./GenerationProgressView"
import { useGenerationProgress } from "./useGenerationProgress"
import type { ConversationTurn, DraftRevision, PluginType, TypeSession, WizardSession } from "./useWizardSession"
import { availableCredentialsFor, canGenerateFor, cleanLinks, isFormComplete } from "./useWizardSession"

/** Triggers the streaming generate call (`generateStream.ts`), gated by `canGenerateFor`, and
 * appends the result as a new `DraftRevision`. Also reused for AI auto-fix via `previousCode`/`previousError`. */
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
  /** Fired whenever this call had a login association to resolve, even if the map is empty
   * (still needs to overwrite a stale prior value). */
  onCredentialValuesResolved: (values: Record<string, string>) => void
  /** Present only for an AI-auto-fix round — `undefined` for a fresh generation. */
  previousCode?: string
  previousError?: string
  label?: string
}) {
  const { t } = useTranslation()
  const canGenerate = canGenerateFor(session, type) && isFormComplete(session, typeSession)
  const origin = previousCode !== undefined ? "ai-auto-fix" : "ai-generated"
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
      // Login only needs its own reference URL, not every shared sample link (sending the full
      // list here caused a generate-timeout).
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
      {generating && <AiSkeleton />}
      <GenerationProgressView items={items} elapsedSeconds={elapsedSeconds} />
    </div>
  )
}

/** The same-domain reference sample the lookup step fetched for a *different* type, e.g. reusing
 * an already-installed "download" plugin's source when generating "metadata". */
function findSameDomainSample(session: WizardSession, type: PluginType): string | undefined {
  const others: PluginType[] = (["login", "metadata", "download"] as PluginType[]).filter((t) => t !== type)
  for (const other of others) {
    const coverage = session.lookupResult[other]
    if (coverage.covered) return coverage.sourceCode
  }
  return undefined
}
