import { Suspense } from "react"
import { useTranslation } from "react-i18next"
import { NavLink, Outlet } from "react-router-dom"

import { Footer, MobileNav, type NavItem } from "@/components/Layout"
import { ReleaseNotesOverlay } from "@/components/Layout"
import { UpdateBanner } from "@/components/Layout"
import { useIsNarrowViewport } from "@/hooks"
import { useSession } from "@/session/SessionProvider"

import { useApplySettingsLanguage } from "./i18n"
import { RouteFallback } from "./RouteFallback"
import { useApplyTheme } from "./theme"

// Legacy's top nav (`<p id="nb">`) is a single centered link line; anonymous nav deliberately
// shows just "Admin Login", not legacy's "Admin Login" + "Statistics". On a phone the signed-in
// line wraps into several rows of small links, so it collapses into `MobileNav`'s drawer there.
export function Layout() {
  const { t } = useTranslation()
  useApplyTheme()
  useApplySettingsLanguage()
  // Do not default to the logged-in link set while the status query is still in flight: the
  // mobile report showed a slow/errored `/login/status` holding the admin nav open over guest
  // content. A brief guest-looking nav is safer than the reverse flash.
  const { isAuthenticated } = useSession()
  const narrow = useIsNarrowViewport("(max-width: 560px)")

  const links: NavItem[] = isAuthenticated
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
    // `relative`: the phone menu button is positioned against this root.
    <div style={{ position: "relative" }}>
      {isAuthenticated && narrow ? (
        <MobileNav links={links} />
      ) : (
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
      )}
      <UpdateBanner />
      <ReleaseNotesOverlay />
      {/* Its own boundary: a page chunk arriving must not blank the navigation bar above it. */}
      <Suspense fallback={<RouteFallback />}>
        <Outlet />
      </Suspense>
      <Footer />
    </div>
  )
}
