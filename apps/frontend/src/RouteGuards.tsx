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

/** Wraps every route expecting an authenticated caller. Unlike `AllowGuest`, it is deliberately
 * not optimistic: rendering an admin page while `login-status` is still loading/errored leaves the
 * admin nav visible over guest-scoped content (the mobile report, 2026-09-04) and, combined with
 * guest-visible API routes such as `GET /bookmarks`, can expose admin data to a caller who is only
 * a guest. Blank until the query settles, then redirect on false/error. */
export function RequireAuth({ children }: { children?: ReactElement }) {
  const loginStatus = useLoginStatus()
  const location = useLocation()

  if (loginStatus.isError) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }
  if (!loginStatus.isSuccess) return null
  if (!loginStatus.data.logged_in) {
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
