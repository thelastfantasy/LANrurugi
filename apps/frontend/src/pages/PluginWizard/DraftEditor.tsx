import { useState } from "react"
import { useTranslation } from "react-i18next"

import { CodeEditor } from "./CodeEditor"
import type { DraftRevision } from "./useWizardSession"

/** Edits are staged locally and only become a new `DraftRevision` on explicit apply — never
 * mutates the existing revision in place, so its trial-run history stays attached. */
export function DraftEditor({
  activeRevision,
  disabled,
  onApply,
}: {
  activeRevision: DraftRevision | undefined
  disabled: boolean
  onApply: (code: string) => void
}) {
  if (!activeRevision) return null
  // Keying on the revision id remounts this inner component to reset local state on change.
  return <DraftEditorInner key={activeRevision.id} activeRevision={activeRevision} disabled={disabled} onApply={onApply} />
}

function DraftEditorInner({
  activeRevision,
  disabled,
  onApply,
}: {
  activeRevision: DraftRevision
  disabled: boolean
  onApply: (code: string) => void
}) {
  const { t } = useTranslation()
  const [draftCode, setDraftCode] = useState(activeRevision.code)
  const isDirty = draftCode !== activeRevision.code

  return (
    <div style={{ marginTop: 8 }}>
      <CodeEditor code={draftCode} onChange={setDraftCode} readOnly={disabled} />
      <input
        type="button"
        className="stdbtn"
        style={{ marginTop: 4 }}
        value={t("pluginWizard.applyEdit") ?? ""}
        disabled={disabled || !isDirty}
        onClick={() => onApply(draftCode)}
      />
    </div>
  )
}
