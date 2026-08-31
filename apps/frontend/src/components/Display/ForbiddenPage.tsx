import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useLoginStatus } from "@/api/hooks"
import { routes } from "@/lib/routes"

/** 403 content, rendered inline wherever a protected page's data load comes back `403` (not a
 * dedicated route). Not-logged-in is a defensive fallback; logged-in shows the real denial reason. */
export function ForbiddenPage({ reason }: { reason?: string }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const loginStatus = useLoginStatus()
  // Defaults to "logged in" while in flight — a 403 already implies some identity was resolved.
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
