import { useState } from "react"
import { useTranslation } from "react-i18next"

import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import { toast } from "@/toast"

import { GenerateStreamError, streamGenerate } from "./generateStream"
import { GenerationProgressView } from "./GenerationProgressView"
import { useGenerationProgress } from "./useGenerationProgress"
import type { ConversationTurn, DraftRevision, PluginType, TypeSession, WizardSession } from "./useWizardSession"
import { availableCredentialsFor, cleanLinks } from "./useWizardSession"

/** User-driven free-text follow-up on an already-generated draft, distinct from the
 * trial-run-failure-driven AI auto-fix in `TrialRunResult.tsx`. */
export function RefinePanel({
  session,
  typeSession,
  type,
  activeRevision,
  isInFlight,
  onBeginOperation,
  onEndOperation,
  onRevisionAppended,
  onConversationTurnAppended,
  onCredentialValuesResolved,
}: {
  session: WizardSession
  typeSession: TypeSession
  type: PluginType
  activeRevision: DraftRevision | undefined
  isInFlight: boolean
  onBeginOperation: () => void
  onEndOperation: () => void
  onRevisionAppended: (revision: DraftRevision) => void
  onConversationTurnAppended: (turn: ConversationTurn) => void
  /** See `TypeSession.resolvedCredentialValues`'s own docs. */
  onCredentialValuesResolved: (values: Record<string, string>) => void
}) {
  const { t } = useTranslation()
  const [instruction, setInstruction] = useState("")
  const [refining, setRefining] = useState(false)
  const { items, elapsedSeconds, start: startProgress, stop: stopProgress, onProgress } = useGenerationProgress()
  const palette = useMenuPalette()

  async function trigger() {
    if (!activeRevision || !instruction.trim()) return
    onBeginOperation()
    setRefining(true)
    startProgress()
    try {
      const loginAssociation = typeSession.loginAssociation
        ? { namespace: typeSession.loginAssociation.namespace }
        : undefined
      const links =
        type === "login" ? [typeSession.loginReferenceUrl].filter(Boolean) : cleanLinks(session.sharedLinks)
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
          refine_instruction: instruction.trim(),
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
        origin: "ai-refine",
        createdAt: Date.now(),
        trialRuns: [],
        explanation: response.explanation,
      })
      onConversationTurnAppended({ userMessage: instruction.trim(), assistantCode: response.code })
      setInstruction("")
    } catch (err) {
      const heading =
        err instanceof GenerateStreamError && err.code === "ai_output_not_code"
          ? t("pluginWizard.aiOutputNotCode")
          : t("pluginWizard.generateFailed")
      toast({ heading: heading ?? undefined, text: String(err), icon: "error" })
    } finally {
      setRefining(false)
      stopProgress()
      onEndOperation()
    }
  }

  return (
    <div
      style={{
        marginTop: 8,
        border: `1px solid ${palette.border}`,
        borderRadius: 4,
        padding: 8,
      }}
    >
      <label style={{ display: "block" }}>
        {t("pluginWizard.refineHint")}
        <textarea
          className="stdinput"
          value={instruction}
          onChange={(e) => setInstruction(e.target.value)}
          placeholder={t("pluginWizard.refinePlaceholder") ?? undefined}
          rows={3}
          style={{ width: "100%", maxWidth: "none", display: "block" }}
        />
      </label>
      <input
        type="button"
        className="stdbtn"
        style={{ marginTop: 4, minWidth: "auto", width: "auto" }}
        value={t("pluginWizard.refineButton") ?? ""}
        disabled={isInFlight || !instruction.trim()}
        onClick={() => void trigger()}
      />
      {refining && <AiSkeleton />}
      <GenerationProgressView items={items} elapsedSeconds={elapsedSeconds} />
    </div>
  )
}
