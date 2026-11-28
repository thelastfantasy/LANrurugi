import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { CollapsibleSection } from "@/components/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

/** Settings entry for the resource monitor. The readings themselves live on their own page
 * (`/resources`): a dozen tables of process and cgroup numbers do not fit an accordion, and the
 * sampling has to start and stop with the page rather than with a section being open. */
export function ResourcesSection() {
  const { t } = useTranslation()
  const navigate = useNavigate()

  return (
    <CollapsibleSection id="resources" icon="fa-gauge-high" title={t("settings.resources.title")}>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("settings.resources.sectionHint")}</p>
      <button type="button" className="stdbtn" onClick={() => navigate(routes.resources())}>
        {t("settings.resources.openPage")}
      </button>
    </CollapsibleSection>
  )
}
