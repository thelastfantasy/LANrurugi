import type { ReactElement } from "react"
import { Navigate, Outlet, useLocation } from "react-router-dom"

import { routes } from "@/lib/routes"
import { useSession } from "@/session/SessionProvider"

/** Wraps a route only for a logged-out visitor (`/login`) — an authenticated session redirects
 * to the library instead. Renders nothing until the shared session status resolves. */
export function RequireGuest({ children }: { children: ReactElement }) {
  const { isSuccess, isAuthenticated } = useSession()

  if (!isSuccess) return null
  if (isAuthenticated) return <Navigate to={routes.library()} replace />

  return children
}

/** Wraps every route expecting an authenticated caller. Unlike `AllowGuest`, it is deliberately
 * not optimistic: rendering an admin page while session status is still loading/errored leaves the
 * admin nav visible over guest-scoped content (the mobile report, 2026-09-04) and, combined with
 * guest-visible API routes such as `GET /bookmarks`, can expose admin data to a caller who is only
 * a guest. Blank until the shared status settles, then redirect on false/error. */
export function RequireAuth({ children }: { children?: ReactElement }) {
  const { isError, isSuccess, isAuthenticated } = useSession()
  const location = useLocation()

  if (isError) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }
  if (!isSuccess) return null
  if (!isAuthenticated) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }

  // Used both wrapping a single element directly and as a parent route (no `children` to pass).
  return children ?? <Outlet />
}

/** Wraps routes an eligible unauthenticated guest may also reach (Library, Reader) — unlike
 * `RequireAuth`, `isGuestModeEnabled` alone is also a valid reason to let the request through. */
export function AllowGuest({ children }: { children?: ReactElement }) {
  const { isSuccess, isAuthenticated, isGuestModeEnabled } = useSession()
  const location = useLocation()

  if (isSuccess && !isAuthenticated && !isGuestModeEnabled) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }

  return children ?? <Outlet />
}
