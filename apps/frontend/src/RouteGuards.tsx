import type { ReactElement } from "react"
import { Navigate, Outlet, useLocation } from "react-router-dom"

import { useLoginStatus } from "@/api/hooks"
import { routes } from "@/lib/routes"

/** Wraps a route that only makes sense for a logged-out visitor (currently just `/login`) — an
 * already-authenticated session is bounced straight to the library instead of seeing the form
 * again. Renders nothing (not the wrapped page, not a fallback) until `/login/status` resolves,
 * then either the real page or the redirect — never both, so a logged-in visitor never sees the
 * form flash before being sent away. */
export function RequireGuest({ children }: { children: ReactElement }) {
  const loginStatus = useLoginStatus()

  if (!loginStatus.isSuccess) return null
  if (loginStatus.data.logged_in) return <Navigate to={routes.library()} replace />

  return children
}

/** Wraps every route that expects an authenticated caller. Session expiry is detected here, not
 * by each page's own data hooks — `client.ts`'s 401 handling no longer force-navigates itself
 * (that was issue #92's own "404/error page flashes for an instant before the redirect lands"
 * bug: a `window.location.assign` doesn't stop the calling component from rendering once more
 * first); it just invalidates the shared `login-status` query, and this is the one place that
 * reacts to it turning false. Keeps every page's own `isError` branch honest — by the time a
 * page's data hook sees an error, a plain 401 has already been intercepted here and never reaches
 * it, so "something else actually broke" is the only case left to explain.
 *
 * Renders the real page optimistically while `/login/status` is still resolving (unlike
 * `RequireGuest`, which can afford to render nothing — a login *form* has no sensitive content to
 * withhold). The alternative — blanking the page until the very first `/login/status` response of
 * a fresh page load — sounds safer but isn't: a genuinely logged-out visitor never actually sees
 * real content during that window, because every data fetch the optimistically-rendered page
 * fires also 401s (same session, same cookie), so all that's visible is the page's own loading
 * skeleton; the same 401 also invalidates `login-status`, so this component immediately redirects
 * once that resolves. A real full-page reload otherwise blanked *every* authenticated route for
 * the round-trip to `/login/status` on every single reload, which is both a visible regression
 * from "route protection didn't exist yet" and, under real latency, worse than the bug it was
 * trying to prevent. */
export function RequireAuth({ children }: { children?: ReactElement }) {
  const loginStatus = useLoginStatus()
  const location = useLocation()

  if (loginStatus.isSuccess && !loginStatus.data.logged_in) {
    return <Navigate to={routes.login()} state={{ from: location }} replace />
  }

  // Used two ways: wrapping a single element directly (`Reader`, which sits outside `Layout`),
  // or as a parent `<Route element={<RequireAuth />}>` for a whole nested route group (every
  // `Layout`-wrapped page) — React Router renders the matched child route via `<Outlet />` in
  // that second case, since there's no single `children` to pass.
  return children ?? <Outlet />
}

/** 007-guest-restricted-access: wraps the handful of routes an eligible unauthenticated guest may
 * also reach (Library, Reader) — unlike `RequireAuth`, `logged_in` alone isn't the gate;
 * `guest_mode_enabled` (the site-wide switch, `login/status`'s own field — true only when the
 * switch is on AND at least one category is `visible_to_guest`, see `procedure.rs`'s own
 * eligibility check this mirrors) is an equally valid reason to let the request through. An
 * authenticated session always passes too, same as `RequireAuth` — this widens who's let in, it
 * doesn't narrow it. Redirects to `/login` only when neither condition holds, matching
 * `RequireAuth`'s own redirect target and optimistic-render behavior (see that component's own
 * docs for why rendering the real page while `/login/status` is still resolving is the right
 * default here too). */
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
