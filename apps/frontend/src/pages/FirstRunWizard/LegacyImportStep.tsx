import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import { waitForJob } from "@/api/client"
import type { ImportLegacyResult } from "@/api/types"
import { Button } from "@/components/common-ui/Form"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

import { FIRST_RUN_LEGACY_IMPORT_URL } from "./logic"
import { StepActions } from "./WizardStepNav"

/** Step 4 — pull metadata in from an existing LANraragi instance's backup export.
 *
 *  Same endpoint as the Backup page's own import (`Backup.tsx`), but deliberately its own smaller
 *  call site rather than a shared hook: this one fixes `merge` + no tag minimization and has no
 *  rollback-snapshot list or repeat-import confirmation, so the only part the two would share is
 *  the four-line POST/`waitForJob` pair. `settings.importFromLegacy*`'s own copy is reused
 *  verbatim, and the `/backup` link below is where the full option set lives. */
export function LegacyImportStep({
  onBack,
  onContinue,
}: {
  onBack: () => void
  onContinue: () => void
}) {
  const { t } = useTranslation()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState("")
  const [result, setResult] = useState<ImportLegacyResult | null>(null)

  async function handleImport(file: File) {
    setBusy(true)
    setStatus(t("settings.importLegacyInProgress"))
    setResult(null)
    try {
      const formData = new FormData()
      formData.append("file", file)
      const response = await fetch(FIRST_RUN_LEGACY_IMPORT_URL, { method: "POST", body: formData })
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)
      if (jobStatus.state === "failed") {
        setStatus(
          t("settings.importLegacyFailedError", {
            error: jobStatus.error ?? t("edit.unknownError"),
          }),
        )
      } else {
        setResult(jobStatus.notes as ImportLegacyResult)
        setStatus(t("settings.importLegacyComplete"))
      }
    } catch (e) {
      setStatus(t("settings.importLegacyFailedError", { error: String(e) }))
    } finally {
      setBusy(false)
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  return (
    <div>
      <h3>{t("settings.importFromLegacyTitle")}</h3>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("settings.importFromLegacyDescription")}</p>

      {busy && (
        <p>
          <i className="fa fa-compact-disc fa-spin" aria-hidden="true"></i> {status}
        </p>
      )}
      {!busy && status && <p style={{ fontSize: FONT_SIZE_SM }}>{status}</p>}

      {result && (
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "auto auto",
            columnGap: 8,
            rowGap: 2,
            width: "fit-content",
            fontSize: FONT_SIZE_SM,
          }}
        >
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyArchivesUpdated")}</span>
          <span>{result.archives_updated}</span>
          <span style={{ textAlign: "right" }}>
            {t("settings.importLegacyArchivesSkippedExisting")}
          </span>
          <span>{result.archives_skipped_already_exists}</span>
          <span style={{ textAlign: "right" }}>
            {t("settings.importLegacyArchivesSkippedNoMatch")}
          </span>
          <span>{result.archives_skipped_no_match}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyCategoriesRestored")}</span>
          <span>{result.categories_restored}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyTankoubonsRestored")}</span>
          <span>{result.tankoubons_restored}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyStampsRestored")}</span>
          <span>{result.stamps_restored}</span>
        </div>
      )}

      <span className="fileinput-button" style={{ display: "block", marginTop: 12 }}>
        <Button style={{ width: "100%", height: 40 }} disabled={busy}>
          <i className="fa fa-upload" aria-hidden="true"></i> {t("settings.importLegacyButton")}
        </Button>
        <input
          ref={fileInputRef}
          type="file"
          name="file"
          accept=".json,application/json"
          disabled={busy}
          onChange={(e) => {
            const file = e.target.files?.[0]
            if (file) void handleImport(file)
          }}
        />
      </span>

      <p style={{ fontSize: FONT_SIZE_SM, opacity: 0.8 }}>
        {t("firstRunWizard.importAdvancedHint")}{" "}
        <Link to={routes.backup()}>{t("settings.databaseBackupRestore")}</Link>
      </p>

      <StepActions
        onBack={onBack}
        onSkip={onContinue}
        skipLabel={t("firstRunWizard.skipStep")}
        primaryLabel={t("pluginWizard.nextStep")}
        onPrimary={onContinue}
        // Only the button is disabled, not spinnered: the running import already has its own
        // spinner in the status line above, and a spinner on "Next" would read as if *advancing*
        // were the thing taking time.
        primaryDisabled={busy}
      />
    </div>
  )
}
