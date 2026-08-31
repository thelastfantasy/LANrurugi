import type { ReactElement } from "react"
import { Navigate, Outlet, useLocation } from "react-router-dom"

import { useLoginStatus } from "@/api/hooks"
import { routes } from "@/lib/routes"

/** Wraps a route only for a logged-out visitor (`/login`) — an authenticated session redirects
 * to the library instead. Renders nothing until `/login/status` resolves. */
export function RequireGuest({ children }: { children: ReactElement }) {
  const loginStatus = useLoginStatus()

  if (!loginStatus.isSuccess) return null
  if (loginStatus.data.logged_in) return <Navigate to={routes.library()} replace />

  return children
}

/** Wraps every route expecting an authenticated caller; renders optimistically while the shared
 * `login-status` query resolves, then redirects if it turns false. */
export function RequireAuth({ children }: { children?: ReactElement }) {
  const loginStatus = useLoginStatus()
  const location = useLocation()

  if (loginStatus.isSuccess && !loginStatus.data.logged_in) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }

  // Used both wrapping a single element directly and as a parent route (no `children` to pass).
  return children ?? <Outlet />
}

/** Wraps routes an eligible unauthenticated guest may also reach (Library, Reader) — unlike
 * `RequireAuth`, `guest_mode_enabled` alone is also a valid reason to let the request through. */
export function AllowGuest({ children }: { children?: ReactElement }) {
  const loginStatus = useLoginStatus()
  const location = useLocation()

  if (
    loginStatus.isSuccess &&
    !loginStatus.data.logged_in &&
    !loginStatus.data.guest_mode_enabled
  ) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }

  return children ?? <Outlet />
}
