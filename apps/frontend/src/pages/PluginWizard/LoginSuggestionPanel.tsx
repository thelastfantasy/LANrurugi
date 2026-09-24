import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { LoginSuggestion, WizardSession } from "./useWizardSession"

/** Shown, dismissibly, when a metadata/download trial run fails and AI judges it may be
 * login-related — offers to associate an existing login plugin or select "login" as a new type. */
export function LoginSuggestionPanel({
  session,
  suggestion,
  onAssociateExisting,
  onSelectLoginType,
}: {
  session: WizardSession
  suggestion: LoginSuggestion
  onAssociateExisting: (namespace: string) => void
  onSelectLoginType: () => void
}) {
  const { t } = useTranslation()
  const [dismissed, setDismissed] = useState(false)
  if (!suggestion.relevant || dismissed) return null

  const existingLogin = session.lookupResult.login
  const loginAlreadySelected = session.selectedTypes.includes("login")

  return (
    <div className="ptbox" style={{ marginTop: 8, padding: 8 }}>
      <p style={{ fontStyle: "italic" }}>
        {t("pluginWizard.loginSuggestionHint")}: {suggestion.reasoning}
      </p>
      {existingLogin.covered ? (
        <input
          type="button"
          className="stdbtn"
          value={t("pluginWizard.useExistingLoginPlugin", { namespace: existingLogin.namespace }) ?? ""}
          onClick={() => onAssociateExisting(existingLogin.namespace)}
        />
      ) : (
        <input
          type="button"
          className="stdbtn"
          value={t("pluginWizard.addLoginPlugin") ?? ""}
          disabled={loginAlreadySelected}
          title={loginAlreadySelected ? (t("pluginWizard.loginTypeAlreadySelected") ?? undefined) : undefined}
          onClick={onSelectLoginType}
        />
      )}
      <input
        type="button"
        className="stdbtn"
        style={{ marginLeft: 8 }}
        value={t("pluginWizard.dismiss") ?? ""}
        onClick={() => setDismissed(true)}
      />
    </div>
  )
}
