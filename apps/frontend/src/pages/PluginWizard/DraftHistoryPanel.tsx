import { useTranslation } from "react-i18next"

import type { TrialRunResult as TrialRunResultData, TypeSession } from "./useWizardSession"

const ORIGIN_LABEL_KEY = {
  "ai-generated": "pluginWizard.originAiGenerated",
  "ai-auto-fix": "pluginWizard.originAiAutoFix",
  "manual-edit": "pluginWizard.originManualEdit",
  "ai-refine": "pluginWizard.originAiRefine",
  "loaded-from-existing": "pluginWizard.originLoadedFromExisting",
} as const

/** Lists every revision for the active type (origin + trial-run outcomes) and lets the user pick
 * any one as `activeRevisionIndex`, independent of recency. */
export function DraftHistoryPanel({
  typeSession,
  onActiveRevisionChanged,
}: {
  typeSession: TypeSession
  onActiveRevisionChanged: (index: number) => void
}) {
  const { t } = useTranslation()

  return (
    <div className="ptbox" style={{ marginTop: 8, padding: 8 }}>
      <h3 className="ih">{t("pluginWizard.draftHistory")}</h3>
      <ul style={{ listStyle: "none", margin: 0, padding: 0 }}>
        {typeSession.revisions.map((revision, index) => {
          const runSucceeded = (r: TrialRunResultData) => {
            if (r.type === "login") return r.outcome === "success"
            // A discovery run has probes rather than per-link results, and one probe returning
            // nothing is not a broken draft — the plugin still works, that hint just matched nothing.
            if (r.type === "discovery") return r.probes.some((p) => p.ok && (p.candidates ?? 0) > 0)
            return r.perLink.every((l) => l.outcome === "success")
          }
          const successCount = revision.trialRuns.filter(runSucceeded).length
          return (
            <li key={revision.id} style={{ marginBottom: 4 }}>
              <label>
                <input
                  type="radio"
                  name={`draft-history-${typeSession.type}`}
                  checked={index === typeSession.activeRevisionIndex}
                  onChange={() => onActiveRevisionChanged(index)}
                />{" "}
                #{index + 1} — {t(ORIGIN_LABEL_KEY[revision.origin])} —{" "}
                {revision.trialRuns.length === 0
                  ? t("pluginWizard.notYetTrialRun")
                  : t("pluginWizard.trialRunCount", { count: revision.trialRuns.length, successCount })}
              </label>
            </li>
          )
        })}
      </ul>
    </div>
  )
}
