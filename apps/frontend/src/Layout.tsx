import { useTranslation } from "react-i18next"
import { NavLink, Outlet } from "react-router-dom"

import { Footer } from "@/components/Layout"
import { UpdateBanner } from "@/components/Layout"

import { useLoginStatus } from "./api/hooks"
import { useApplySettingsLanguage } from "./i18n"
import { useApplyTheme } from "./theme"

// Legacy's own top nav (`~/LANraragi/templates/index.html.tt2`'s `<p id="nb">`, reused across
// every page) is a single centered text line of links — Font Awesome caret icons separating bold
// links, styled by `p#nb`/`p#nb a` in the copied theme CSS (`useApplyTheme`) — but the *set* of
// links depends on `[% IF userlogged %]`: six links when logged in. Legacy itself shows "Admin
// Login" + "Statistics" when logged out (verified against the real template) — deliberately NOT
// matched here: the anonymous nav is just "Admin Login" alone, since surfacing a link to a page
// that isn't the login flow itself reads as more functionality than an unauthenticated visitor
// actually has available, not a legacy-parity requirement worth keeping.
// Batch Operations/Plugin Configuration/Database Backup-Restore aren't here because legacy
// doesn't put them here either — they're buttons on the Settings page itself
// (`~/LANraragi/templates/config.html.tt2`'s left column: `#plugin-config`/`#backup`/`#batch`),
// which is also where the language switcher and logout now live (see `Settings.tsx`) — legacy's
// nav has no room for either.
export function Layout() {
  const { t } = useTranslation()
  useApplyTheme()
  useApplySettingsLanguage()
  const loginStatus = useLoginStatus()
  // Defaults to the logged-in link set while the status query is still in flight (its very first
  // load) rather than flashing the reduced anonymous nav for a moment — `loginStatus.data` is
  // `undefined` only during that brief window, never once a result (true or false) has landed.
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
