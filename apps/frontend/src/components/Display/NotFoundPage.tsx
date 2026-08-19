import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { routes } from "@/lib/routes"

/** issue #92's own 404 content — reused both as the catch-all route's own full-page render
 * (`App.tsx`'s `path="*"`, an unknown URL the visitor actually navigated/linked to) and inline
 * inside `Reader.tsx` when a specific archive/Tankoubon id 404s (that case deliberately does NOT
 * navigate anywhere — the URL stays `/reader/<id>`, only the rendered content changes, so a
 * visitor who bookmarked or shared that exact link still sees the same URL that produced the 404,
 * not a generic `/404`). `.ido` (not `Layout`-specific styling) so it renders correctly whether or
 * not a caller has already wrapped it in `Layout` — the catch-all route itself is NOT nested under
 * `Layout` (matching `Reader`'s own top-level, nav-less placement — see `App.tsx`), so this
 * component provides its own minimal shell. */
export function NotFoundPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  return (
    <div className="ido" style={{ textAlign: "center", padding: 40 }}>
      <i className="fas fa-8x fa-compass" aria-hidden="true"></i>
      <h2 style={{ marginTop: 16 }}>{t("common.notFoundTitle")}</h2>
      <p>{t("common.notFoundMessage")}</p>
      <input
        type="button"
        className="stdbtn"
        style={{ marginTop: 16 }}
        value={t("common.returnToLibrary") ?? undefined}
        onClick={() => navigate(routes.library())}
      />
    </div>
  )
}
