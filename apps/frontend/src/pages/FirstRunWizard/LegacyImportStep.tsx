import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import { routes } from "@/lib/routes"
import { LegacyImportPanel } from "@/pages/Settings/LegacyImportPanel"
import { FONT_SIZE_SM } from "@/theme"

import { FIRST_RUN_IMPORT_CONFLICT_MODE } from "./logic"
import { StepActions } from "./WizardStepNav"

/** Step 4 — pull metadata in from an existing LANraragi instance's backup export, through the same
 *  panel as the Backup page so the two offer identical choices; only the starting conflict mode
 *  differs (see `FIRST_RUN_IMPORT_CONFLICT_MODE`). */
export function LegacyImportStep({
  onBack,
  onContinue,
}: {
  onBack: () => void
  onContinue: () => void
}) {
  const { t } = useTranslation()
  const [busy, setBusy] = useState(false)

  return (
    <div>
      <LegacyImportPanel defaultConflictMode={FIRST_RUN_IMPORT_CONFLICT_MODE} onBusyChange={setBusy} />

      <p style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, textAlign: "center" }}>
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
        // spinner in the panel above, and a spinner on "Next" would read as if *advancing* were
        // the thing taking time.
        primaryDisabled={busy}
      />
    </div>
  )
}
