import { useState } from "react"
import { useTranslation } from "react-i18next"

import { CodeEditor } from "./CodeEditor"
import type { DraftRevision } from "./useWizardSession"

/** T033/T034 (US4) — manual code editing. An edit is staged locally in this component (not pushed
 * to `useWizardSession` on every keystroke) and only becomes a new `DraftRevision`
 * (`origin: "manual-edit"`, empty `trialRuns`) when the user explicitly applies it — editing never
 * mutates the existing revision in place, so its trial-run history stays attached to it
 * unaffected (US4 AC1). */
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
  // Keying on the revision's own id remounts this inner component (and its `useState`) whenever
  // the active revision changes identity — React's own recommended way to reset local state on a
  // prop change, instead of a setState-in-effect resync.
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
