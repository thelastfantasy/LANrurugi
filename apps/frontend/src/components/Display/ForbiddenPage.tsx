import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useLoginStatus } from "@/api/hooks"
import { routes } from "@/lib/routes"

/** issue #92's own 403 content — rendered inline wherever a protected page/operation's own data
 * load comes back `403` (not navigated to a dedicated `/403` route — same "stay on the URL that
 * produced the error" principle `NotFoundPage` follows). Branches on `useLoginStatus()`:
 *
 * - Not logged in: a 403 here almost never happens in practice — the only caller (`Reader.tsx`)
 *   sits behind `RequireAuth` (`RouteGuards.tsx`), which already redirects an unauthenticated
 *   visitor to `/login` before this component ever mounts, and the backend's own `require_api_key`
 *   middleware (`crates/lanrurugi-api/src/procedure.rs`) returns `401`, not `403`, for "nobody at
 *   all" regardless. This branch is kept as a defensive fallback for a stale-session edge case
 *   (`loginStatus` itself briefly reporting "not logged in" right as a session expires
 *   mid-page-view, between `RequireAuth`'s own check and this render) rather than assuming it
 *   can't happen. Points at the login page rather than trying to explain a denial reason that
 *   isn't really about permissions.
 * - Logged in (including a real Guest-role API token, which the frontend has no way to positively
 *   identify — see `reason` below): shows the backend's own real denial reason instead of a canned
 *   message, since "logged in but still forbidden" always has a specific cause (route policy,
 *   Guest-role restriction, etc.) that's more useful surfaced verbatim than genericized. */
export function ForbiddenPage({ reason }: { reason?: string }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const loginStatus = useLoginStatus()
  // Defaults to "logged in" while the status query is still in flight, same reasoning as
  // `Layout.tsx`'s own `loggedIn` default — a 403 already implies *some* identity was resolved
  // server-side (a real 401 would have intercepted first), so "not logged in" is never the more
  // likely guess for the brief loading window.
  const loggedIn = loginStatus.data?.logged_in ?? true

  return (
    <div className="ido" style={{ textAlign: "center", padding: 40 }}>
      <i className="fas fa-8x fa-lock" aria-hidden="true"></i>
      <h2 style={{ marginTop: 16 }}>{t("common.forbiddenTitle")}</h2>
      {loggedIn ? (
        <p>{reason ?? t("common.forbiddenLoggedInMessage")}</p>
      ) : (
        <p>{t("common.forbiddenLoggedOutMessage")}</p>
      )}
      <div style={{ marginTop: 16, display: "flex", gap: 8, justifyContent: "center" }}>
        {!loggedIn && (
          <input
            type="button"
            className="stdbtn"
            value={t("app.adminLogin") ?? undefined}
            onClick={() => navigate(routes.login())}
          />
        )}
        <input
          type="button"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    </div>
  )
}
