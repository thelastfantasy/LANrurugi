import { useEffect } from "react"
import { useLocation, useNavigate } from "react-router-dom"

import { routes } from "@/lib/routes"
import { useSession } from "@/session/SessionProvider"

/** The only path the wizard auto-opens from: the library landing page `Login.tsx` navigates to
 * after a successful sign-in. Deliberately not "any admin page" — someone who deep-linked
 * straight into a specific page (a bookmark to `/upload`, a bridge return URL) asked for that
 * page, not for onboarding. */
export function isFirstRunWizardTriggerPath(pathname: string): boolean {
  return pathname === "/"
}

/** Everything `FirstRunWizardGate` needs to decide, as a plain function so the matrix (pending vs.
 * not, authenticated vs. guest, landing page vs. elsewhere) is unit-testable without a router. */
export function shouldOpenFirstRunWizard({
  pathname,
  sessionResolved,
  isAuthenticated,
  firstRunWizardPending,
  alreadyOpenedThisPageLoad,
}: {
  pathname: string
  sessionResolved: boolean
  isAuthenticated: boolean
  firstRunWizardPending: boolean
  alreadyOpenedThisPageLoad: boolean
}): boolean {
  if (alreadyOpenedThisPageLoad) return false
  if (!sessionResolved) return false
  if (!isAuthenticated) return false
  if (!firstRunWizardPending) return false
  return isFirstRunWizardTriggerPath(pathname)
}

/** One auto-open per page load. Without this, leaving `/welcome` by any means the wizard doesn't
 * itself control (browser Back, a history entry restored from a bfcache) would land back on `/`
 * and be redirected straight into the wizard again — with `replace: true` below that history
 * entry is usually gone, but this makes the property hold regardless of how the user got back.
 * A full reload resets it, so an unhandled wizard still returns on the next visit. */
let autoOpenedThisPageLoad = false

/**
 * Redirects an admin whose instance has never run the first-run wizard into `/welcome`, once.
 *
 * Navigation, not rendering: the wizard is its own full route (it edits the admin password and
 * the instance's LLM key, neither of which belongs in an overlay over a live library page).
 * `replace` keeps the pre-wizard `/` entry out of history — pressing Back out of the wizard
 * should not land the user on a page that immediately bounces them forward again.
 */
export function FirstRunWizardGate() {
  const location = useLocation()
  const navigate = useNavigate()
  const { isSuccess, isAuthenticated, firstRunWizardPending } = useSession()

  useEffect(() => {
    const shouldOpen = shouldOpenFirstRunWizard({
      pathname: location.pathname,
      // `isSuccess` alone, not `isPending`: an errored status must not be treated as "resolved,
      // no wizard" any more than it is by `RequireAuth` — both simply wait for a real answer.
      sessionResolved: isSuccess,
      isAuthenticated,
      firstRunWizardPending,
      alreadyOpenedThisPageLoad: autoOpenedThisPageLoad,
    })
    if (!shouldOpen) return
    autoOpenedThisPageLoad = true
    navigate(routes.welcome(), { replace: true })
  }, [location.pathname, isSuccess, isAuthenticated, firstRunWizardPending, navigate])

  return null
}
