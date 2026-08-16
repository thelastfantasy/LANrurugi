import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { LOG_CATEGORIES, type LogCategory, useLogLines } from "@/api/hooks"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { useApplyTheme } from "@/theme"

const CATEGORY_LABELS: Record<LogCategory, string> = {
  general: "General",
  shinobu: "Shinobu",
  plugins: "Plugins",
  redis: "Redis",
  http: "HTTP",
}

// Matches legacy's own button copy exactly (`logs.html.tt2`'s `#show-*` buttons) for every
// category except `http` (legacy calls the equivalent category `mojo`, after its own underlying
// web framework — this project runs Axum, not Mojolicious, so the button copy names the protocol
// instead of a framework this project doesn't use — issue #86), distinct from `CATEGORY_LABELS`
// above (which is just the bare category name, used for the "Currently Viewing:" indicator).
const CATEGORY_BUTTON_LABELS: Record<LogCategory, string> = {
  general: "View LANrurugi Logs",
  shinobu: "View Shinobu Logs",
  plugins: "View Plugin Logs",
  redis: "View Redis Logs",
  http: "View HTTP Request Logs",
}

// Mirrors legacy's `~/LANraragi/templates/logs.html.tt2` — the intro paragraph + per-category
// bullet list, `.ih` heading with a floated refresh icon and a "Lines:" count input, and the log
// body itself is a one-row `table.itg` (`tr.gtr1 > td > pre.log-panel`), not a styled card.
// Doesn't reproduce the live-tailing/auto-refresh behavior (`logs.js`'s polling) — this refetches
// on category/line-count change and via the refresh icon only.
export function Logs() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const [category, setCategory] = useState<LogCategory>("general")
  const [lines, setLines] = useState(100)
  const logLines = useLogLines(category, lines)
  useApplyTheme()
  useDocumentTitle(t("app.logs") ?? undefined)

  return (
    <div className="ido" style={{ textAlign: "center" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("logs.applicationLogs")}
      </h2>

      <br />
      {t("logs.youCanCheckLanrurugiLogs")}
      <br />
      {t("logs.byDefaultThisViewOnly")}
      <br />
      <br />
      <ul>
        <li>{t("logs.generalLogsPertainToThe")}</li>
        <li>{t("logs.shinobuLogsCorrespondToThe")}</li>
        <li>{t("logs.pluginLogsAreReservedFor")}</li>
        <li>{t("logs.httpLogsWonTTell")}</li>
        <li>{t("logs.redisLogsWonTBe")}</li>
      </ul>
      <br />
      <br />

      <h1 className="ih" style={{ float: "left", marginLeft: "5%" }}>
        {t("logs.currentlyViewing")} <span id="indicator">{t(CATEGORY_LABELS[category])}</span>
      </h1>
      <div style={{ marginRight: "5%", float: "right" }}>
        <a
          href="#"
          title="Refresh"
          onClick={(e) => {
            e.preventDefault()
            void logLines.refetch()
          }}
        >
          <i style={{ paddingRight: 10 }} className="fa fa-sync-alt fa-2x"></i>
        </a>
        {t("logs.lines")}{" "}
        <input
          type="number"
          min={0}
          value={lines}
          onChange={(e) => setLines(Math.max(0, Number(e.target.value) || 0))}
          style={{ width: 60 }}
        />
      </div>
      <div style={{ clear: "both" }} />

      <table className="itg" style={{ width: "100%", marginTop: 32 }}>
        <tbody>
          <tr className="gtr1">
            <td>
              <pre id="log-container" className="log-panel">
                {logLines.isLoading ? t("common.loadingLibrary") : logLines.data || t("logs.noLogsToBeFound")}
              </pre>
            </td>
          </tr>
        </tbody>
      </table>

      <span id="buttonstagging">
        {LOG_CATEGORIES.map((cat) => (
          <input
            key={cat}
            type="button"
            className="stdbtn"
            value={t(CATEGORY_BUTTON_LABELS[cat]) ?? undefined}
            onClick={() => setCategory(cat)}
          />
        ))}
      </span>
      <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
