import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useLogin } from "../api/hooks"
import { Footer } from "../components/Footer"
import { routes } from "../routes"
import { FONT_SIZE_8PT, useApplyTheme } from "../theme"
import { useDocumentTitle } from "../useDocumentTitle"

// Mirrors legacy's `~/LANraragi/templates/login.html.tt2` line-for-line: a plain centered `.ido`
// form, "Admin Password:"/input on one table row (not stacked), the wrong-password row only when
// `wrongpass` is set — verified against the real template, not the index page's separate "no-pass"
// redirect text this previously (incorrectly) reused. `[% INCLUDE footer %]` sits *outside* the
// closing `</div>` of `.ido` in the real template (a body-level sibling, not nested inside the
// card), so `<Footer />` here is a sibling of `.ido`, not a child of it.
export function Login() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const login = useLogin()
  const [password, setPassword] = useState("")
  useApplyTheme()
  useDocumentTitle(t("Admin Login") ?? undefined)

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
        <p>{t("This page requires you to log on.")}</p>

        <form onSubmit={handleSubmit} name="loginForm" method="post">
          <table style={{ margin: "auto", textAlign: "left", fontSize: FONT_SIZE_8PT }}>
            <tbody>
              <tr>
                <td>{t("Admin Password:")}</td>
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
                    value={t("Login") ?? undefined}
                    style={{ width: 60 }}
                  />
                </td>
              </tr>
              {login.isError && (
                <tr style={{ fontSize: 23 }}>
                  <td colSpan={2} style={{ paddingTop: 5, textAlign: "center", verticalAlign: "middle" }}>
                    {t("Wrong Password.")}
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </form>
      </div>

      <Footer />
    </>
  )
}
