import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate, useSearchParams } from "react-router-dom"

import { useLogin } from "@/api/hooks"
import { Footer } from "@/components/Layout"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { useApplyTheme } from "@/theme"

/**
 * The admin login form.
 *
 * Deliberately *not* legacy's `login.html.tt2` markup any more: that page put the label and the
 * field in adjacent `<td>`s of a centered table, which lines nothing up (the label is unbounded
 * while the field is a fixed 90% of its cell) and made both buttons look like full-width bars.
 *
 * Everything visual — the soft accent shapes behind the card, the card surface, control heights —
 * lives in each theme's own theme file (`.login-*`), so this stays inside the app's existing look
 * and follows all five themes. Route protection lives in `RequireGuest` (`App.tsx`), not here.
 */
export function Login() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [searchParams] = useSearchParams()
  const login = useLogin()
  const [password, setPassword] = useState("")
  useApplyTheme({ preferAdminTheme: true })
  useDocumentTitle(t("app.adminLogin") ?? undefined)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    try {
      await login.mutateAsync(password)
      const next = searchParams.get("next")
      if (next && next.startsWith("/") && !next.startsWith("//")) {
        // Internal bridge/start URL: let the server redirect through the SSO handoff.
        window.location.assign(next)
      } else {
        navigate(routes.library())
      }
    } catch {
      // login.isError renders the message below; nothing else to do here.
    }
  }

  const ssoError = searchParams.get("sso_error")

  return (
    <>
      <div className="login-shell">
        <form onSubmit={handleSubmit} name="loginForm" method="post" className="login-card">
          <h1 className="login-title">{t("app.adminLogin")}</h1>
          <p className="login-hint">{t("login.thisPageRequiresYouTo")}</p>

          <label className="login-label" htmlFor="pw_field">
            {t("login.adminPassword")}
          </label>
          <input
            id="pw_field"
            type="password"
            autoFocus
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="stdinput login-input"
            maxLength={255}
            size={20}
            name="password"
          />

          {(login.isError || ssoError) && (
            <p className="login-error">
              {login.isError ? t("login.wrongPassword") : t("login.ssoError")}
            </p>
          )}

          <input
            type="submit"
            className="stdbtn login-submit"
            disabled={login.isPending}
            value={t("library.login") ?? undefined}
          />

          <button type="button" className="login-back" onClick={() => navigate(routes.library())}>
            {t("common.returnToLibrary")}
          </button>
        </form>
      </div>

      <Footer />
    </>
  )
}
