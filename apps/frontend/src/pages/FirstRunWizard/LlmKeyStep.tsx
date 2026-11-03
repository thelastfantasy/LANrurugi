import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useSettings } from "@/api/hooks"
import { Input } from "@/components/common-ui/Form"
import { FONT_SIZE_SM } from "@/theme"

import { StepActions } from "./WizardStepNav"

/** Step 3 — store a DeepSeek API key.
 *
 *  Controlled by the wizard's own draft state (see `PasswordStep`'s own note), and the commit lives
 *  in the container so the last page's "submit everything outstanding" button can reuse it — the
 *  key is never echoed back by the server, so a draft left un-submitted here is a key the admin
 *  typed and would otherwise silently lose. */
export function LlmKeyStep({
  value,
  onChange,
  onSubmit,
  onBack,
  onContinue,
}: {
  value: string
  onChange: (value: string) => void
  /** Resolves to an error message, or `null` when the draft was written (or there was nothing to
   *  write). */
  onSubmit: () => Promise<string | null>
  onBack: () => void
  onContinue: () => void
}) {
  const { t } = useTranslation()
  const settings = useSettings()
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)

  const keyIsSet = settings.data?.llm_api_key_set === true

  async function handleNext() {
    setError("")
    setBusy(true)
    try {
      const message = await onSubmit()
      if (message) {
        setError(message)
        return
      }
      onContinue()
    } finally {
      setBusy(false)
    }
  }

  return (
    <div>
      <h3>{t("firstRunWizard.llmHeading")}</h3>

      <div className={keyIsSet ? "wizard-callout-done" : "wizard-callout-todo"}>
        <i className={keyIsSet ? "fa fa-check" : "fa fa-key"} aria-hidden="true"></i>{" "}
        {keyIsSet ? t("firstRunWizard.llmKeySet") : t("firstRunWizard.llmKeyMissing")}
      </div>

      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.llmBody")}</p>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.llmFeatures")}</p>

      <label htmlFor="first-run-llm-key" style={{ display: "block", fontSize: FONT_SIZE_SM }}>
        {t("settings.deepseekApiKey")}
      </label>
      <Input
        id="first-run-llm-key"
        style={{ width: "100%" }}
        type="password"
        autoComplete="off"
        placeholder="sk-..."
        value={value}
        onValueChange={onChange}
      />

      <p style={{ fontSize: FONT_SIZE_SM, opacity: 0.8 }}>
        {t("firstRunWizard.llmWhereToGet")}
        <br />
        {t("firstRunWizard.llmBackfillNote")}
      </p>

      {error && (
        <p style={{ fontSize: FONT_SIZE_SM }} role="alert">
          {error}
        </p>
      )}

      <StepActions
        onBack={onBack}
        onSkip={onContinue}
        skipLabel={t("firstRunWizard.skipStep")}
        primaryLabel={t("pluginWizard.nextStep")}
        onPrimary={() => void handleNext()}
        busy={busy}
      />
    </div>
  )
}
