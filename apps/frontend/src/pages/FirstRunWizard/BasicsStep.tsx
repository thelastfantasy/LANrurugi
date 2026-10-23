import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useSettings } from "@/api/hooks"
import { Input } from "@/components/common-ui/Form"
import { TimezoneSelect } from "@/components/Form"
import { LanguageOrderEditor } from "@/pages/Settings/LanguageOrderEditor"
import { useSession } from "@/session/SessionProvider"
import { FONT_SIZE_SM } from "@/theme"

import { StepActions } from "./WizardStepNav"

/** The wizard's first data-entry page: admin password, interface language, timezone.
 *
 *  Three unrelated settings on one page because they are the three a brand-new instance cannot
 *  sensibly be used without — and the language picker in particular has to be reachable *before*
 *  anything else is readable, which is why it lives on the first page rather than on a later one.
 *
 *  Controlled by the wizard's own draft state rather than local state, so whatever is typed here
 *  survives jumping to another step and back. The commits themselves live in the container
 *  (`submitPassword` / `submitTimeZone`) because the last page's "submit everything outstanding"
 *  button has to run the same writes. */
export function BasicsStep({
  password,
  confirmation,
  onPasswordChange,
  onConfirmationChange,
  language,
  onLanguageChange,
  timezone,
  onTimezoneChange,
  onSubmit,
  onBack,
  onContinue,
}: {
  password: string
  confirmation: string
  onPasswordChange: (value: string) => void
  onConfirmationChange: (value: string) => void
  /** The ordered preference (`"auto"`, or `"ja,en"`), from the same editor the Settings page uses.
   *  `null` means "untouched" — the server's current value stays in effect. */
  language: string | null
  onLanguageChange: (value: string) => void
  /** `null` means "untouched" — the server's current value stays in effect. */
  timezone: string | null
  onTimezoneChange: (value: string) => void
  /** Resolves to an error message, or `null` when the drafts were written (or there was nothing to
   *  write). */
  onSubmit: () => Promise<string | null>
  onBack: () => void
  onContinue: () => void
}) {
  const { t } = useTranslation()
  const { usingDefaultPassword } = useSession()
  const settings = useSettings()
  const [error, setError] = useState("")
  const [busy, setBusy] = useState(false)

  const effectiveLanguage = language ?? settings.data?.language ?? "auto"
  const effectiveTimezone = timezone ?? settings.data?.timezone ?? "UTC"

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
      <h3>{t("firstRunWizard.passwordHeading")}</h3>

      <div className={usingDefaultPassword ? "wizard-callout-todo" : "wizard-callout-done"}>
        <i
          className={usingDefaultPassword ? "fa fa-exclamation-triangle" : "fa fa-check"}
          aria-hidden="true"
        ></i>{" "}
        {usingDefaultPassword
          ? t("firstRunWizard.passwordStillDefault")
          : t("firstRunWizard.passwordAlreadySet")}
      </div>

      <p style={{ fontSize: FONT_SIZE_SM }}>{t("firstRunWizard.passwordBody")}</p>

      <label htmlFor="first-run-new-password" style={{ display: "block", fontSize: FONT_SIZE_SM }}>
        {t("settings.newPassword")}
      </label>
      <Input
        id="first-run-new-password"
        style={{ width: "100%" }}
        maxLength={255}
        type="password"
        autoComplete="new-password"
        value={password}
        onValueChange={onPasswordChange}
      />

      <label
        htmlFor="first-run-new-password-confirm"
        style={{ display: "block", fontSize: FONT_SIZE_SM, marginTop: 8 }}
      >
        {t("settings.newPasswordConfirmation")}
      </label>
      <Input
        id="first-run-new-password-confirm"
        style={{ width: "100%" }}
        maxLength={255}
        type="password"
        autoComplete="new-password"
        value={confirmation}
        onValueChange={onConfirmationChange}
      />

      <h3 style={{ marginTop: 24 }}>{t("settings.language")}</h3>
      {/* The Settings page's own editor (automatic toggle + ordered fallback list), not the sidebar's
          one-click selector: this page is the only chance to set the preference before anything else
          is configured, and the order is what decides both the interface language and a work title's
          language. It is a draft like everything else here — committed by Next, not on each click,
          exactly as it behaves inside the Settings form. */}
      <LanguageOrderEditor value={effectiveLanguage} onChange={onLanguageChange} />
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("settings.selectTheLanguageForThe")}</p>

      <h3 style={{ marginTop: 24 }}>{t("settings.timezone")}</h3>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("settings.ianaTimezoneIdentifierEG")}</p>
      <TimezoneSelect
        value={effectiveTimezone}
        onChange={onTimezoneChange}
        ariaLabel={t("settings.timezone")}
      />

      {error && (
        <p style={{ fontSize: FONT_SIZE_SM, marginTop: 8 }} role="alert">
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
