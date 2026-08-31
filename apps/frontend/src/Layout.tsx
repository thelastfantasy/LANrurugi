import { useTranslation } from "react-i18next"
import { NavLink, Outlet } from "react-router-dom"

import { Footer } from "@/components/Layout"
import { UpdateBanner } from "@/components/Layout"

import { useLoginStatus } from "./api/hooks"
import { useApplySettingsLanguage } from "./i18n"
import { useApplyTheme } from "./theme"

// Legacy's top nav (`<p id="nb">`) is a single centered link line; anonymous nav deliberately
// shows just "Admin Login", not legacy's "Admin Login" + "Statistics".
export function Layout() {
  const { t } = useTranslation()
  useApplyTheme()
  useApplySettingsLanguage()
  const loginStatus = useLoginStatus()
  // Defaults to the logged-in link set while the status query is still in flight, to avoid
  // flashing the reduced anonymous nav for a moment.
  const loggedIn = loginStatus.data?.logged_in ?? true

  const links: Array<{ to: string; label: string; end?: boolean }> = loggedIn
    ? [
        { to: "/upload", label: t("app.addArchives") },
        { to: "/duplicates", label: t("app.duplicateDetection") },
        { to: "/config", label: t("app.settings") },
        { to: "/config/categories", label: t("app.modifyCategories") },
        { to: "/stats", label: t("app.statistics") },
        { to: "/logs", label: t("app.logs") },
        { to: "/activity", label: t("app.activity") },
        { to: "/bookmarks", label: t("app.bookmarks") },
      ]
    : [{ to: "/login", label: t("app.adminLogin") }]

  return (
    <div>
      <p id="nb">
        {links.map((link, i) => (
          <span key={link.to}>
            <i className="fa fa-caret-right"></i>
            <NavLink to={link.to} end={link.end}>
              {link.label}
            </NavLink>
            {i < links.length - 1 && <span style={{ marginLeft: 5 }}></span>}
          </span>
        ))}
      </p>
      <UpdateBanner />
      <Outlet />
      <Footer />
    </div>
  )
}
