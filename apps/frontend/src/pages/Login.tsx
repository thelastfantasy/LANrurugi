import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useLogin } from "@/api/hooks"
import { Footer } from "@/components/Layout"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_XS, useApplyTheme } from "@/theme"

/** Mirrors legacy's `login.html.tt2` plus the same return-to-library affordance used by other
 * admin pages. Route protection lives in `RequireGuest` (`App.tsx`), not here. */
export function Login() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const login = useLogin()
  const [password, setPassword] = useState("")
  useApplyTheme({ preferAdminTheme: true })
  useDocumentTitle(t("app.adminLogin") ?? undefined)

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    try {
      await login.mutateAsync(password)
      navigate(routes.library())
    } catch {
      // login.isError renders the message below; nothing else to do here.
    }
  }

  return (
    <>
      <div className="ido" style={{ textAlign: "center" }}>
        <p>{t("login.thisPageRequiresYouTo")}</p>

        <form onSubmit={handleSubmit} name="loginForm" method="post">
          <table style={{ margin: "auto", textAlign: "left", fontSize: FONT_SIZE_XS }}>
            <tbody>
              <tr>
                <td>{t("login.adminPassword")}</td>
                <td>
                  <input
                    id="pw_field"
                    type="password"
                    autoFocus
                    value={password}
                    onChange={(e) => setPassword(e.target.value)}
                    className="stdinput"
                    style={{ width: "90%" }}
                    maxLength={255}
                    size={20}
                    name="password"
                  />
                </td>
              </tr>
              <tr>
                <td colSpan={2} style={{ paddingTop: 5, textAlign: "center", verticalAlign: "middle" }}>
                  <input
                    type="submit"
                    className="stdbtn"
                    disabled={login.isPending}
                    value={t("library.login") ?? undefined}
                    style={{ width: 60 }}
                  />
                </td>
              </tr>
              {login.isError && (
                <tr style={{ fontSize: 23 }}>
                  <td colSpan={2} style={{ paddingTop: 5, textAlign: "center", verticalAlign: "middle" }}>
                    {t("login.wrongPassword")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </form>

        <br />
        <input
          type="button"
          id="return"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>

      <Footer />
    </>
  )
}
