import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { LoginSuggestion, WizardSession } from "./useWizardSession"

/** T041/T042/T043 (US7/FR-025) — shown when a metadata/download trial run fails and AI judges the
 * failure might be login-related (`login_suggestion.relevant`). Dismissible (spec Edge Cases —
 * the user must be able to ignore it), and offers a real "add a login plugin" entry point:
 * - if the domain already has a covered login plugin, associates it directly, no generation needed
 * - otherwise selects "login" as a type (reusing US1's per-type input collection / US2's generate
 *   flow the same way the up-front FR-013 path does) so the user can generate/trial-run one now
 */
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
