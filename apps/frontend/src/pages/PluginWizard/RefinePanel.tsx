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

/** User-driven free-text follow-up on an already-generated draft (e.g. "帮我加上从 source tag
 * 提取 ID 的回退逻辑") — distinct from US5's AI-auto-fix (`TrialRunResult.tsx`'s own
 * `triggerAutoFix`), which is trial-run-failure-driven and needs no user input. This is available
 * any time there's an active revision, success or failure, since it's not fixing anything —
 * `TypeSession.conversationHistory` (every past round's own ask+code, replayed by the backend)
 * is what lets this build on everything already discussed rather than the model only ever seeing
 * the latest code snapshot in isolation. */
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
  // Real per-theme colors (`MENU_PALETTE`, already used elsewhere for exactly this "bordered card
  // that isn't a legacy element" case — `PopupMenu.tsx`), not `.ptbox` — that class carries no
  // actual CSS rule in any of the 5 themes (verified), so every other `.ptbox` usage in this same
  // wizard reads as unbordered whitespace, not a real card. This one gets a real border/background
  // specifically because the user asked for this section to read as visually separated (real
  // feedback, 2026-08-26).
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
          // `.stdinput`'s own theme CSS caps `max-width: 450px` — override so this textarea can
          // actually reach the wizard's own 720px step container width, same fix as
          // `SharedLinksForm.tsx`'s own textarea.
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
