import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { Button } from "@/components/common-ui/Form"
import { routes } from "@/lib/routes"
import { useSession } from "@/session/SessionProvider"

/**
 * The Settings page's "run the setup wizard again" action.
 *
 * Renders nothing at all on a deployment that switched the wizard off
 * (`--disable-first-run-wizard` / `LANRURUGI_DISABLE_FIRST_RUN_WIZARD`): the wizard route itself
 * bounces straight back to the library there, so offering the button would just be a dead end.
 *
 * Its own component (rather than an inline conditional in `SettingsPage`) so both branches are
 * reachable from a test without standing up the whole Settings page's hook surface.
 */
export function WizardRerunButton() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { firstRunWizardAvailable } = useSession()

  if (!firstRunWizardAvailable) return null

  return (
    <Button id="first-run-wizard" onClick={() => navigate(routes.welcome("settings"))}>
      {t("firstRunWizard.rerunButton")}
    </Button>
  )
}
