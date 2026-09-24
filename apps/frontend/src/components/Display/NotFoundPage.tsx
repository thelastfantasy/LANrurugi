import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { routes } from "@/lib/routes"

/** Shared 404 content, used both as the catch-all route and inline in `Reader.tsx` (URL stays
 * unchanged there so a bookmarked link still resolves). */
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
