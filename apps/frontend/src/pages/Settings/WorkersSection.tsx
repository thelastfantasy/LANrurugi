import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useShinobuStatus } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, Row } from "./shared"

/** Status only: the watcher recovers by itself (and scans for what it missed), so there is no
 * restart button; a library on a network share, whose changes may never raise events, is
 * covered by "Rescan archive directory". `/api/shinobu/restart` stays for legacy clients. */
export function WorkersSection() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const shinobuStatus = useShinobuStatus()
  const status = shinobuStatus.data
  // Older servers report only `is_alive`.
  const watcherState = status?.state ?? (status?.is_alive ? "running" : "stopped")

  return (
    <CollapsibleSection id="background-workers" icon="fa-satellite" title={t("settings.backgroundWorkers")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          <Row label={t("settings.fileWatcherStatus")}>
            <span>
              {t("settings.fileWatcherIs")}{" "}
              {watcherState === "running" ? (
                <h2 className="ih" style={{ display: "inline", color: "rgb(26, 165, 26)" }}>
                  👍 {t("settings.ok")}
                </h2>
              ) : watcherState === "recovering" ? (
                <h2 className="ih watcher-recovering" style={{ display: "inline" }}>
                  🔧 {t("settings.fileWatcherRecovering")}
                </h2>
              ) : (
                <h2 className="ih" style={{ display: "inline", color: "rgb(207, 37, 37)" }}>
                  👹 {t("settings.kaput")}
                </h2>
              )}
            </span>{" "}
            {/* No PID: the watcher is a task inside the server process, so the API's `pid` (kept
                for legacy clients) is just the server's own. */}
            {t("settings.fileWatcherInProcess")}
            <br />
            {t("settings.fileWatcherDescription")} {t("settings.fileWatcherSelfHeals", { button: t("settings.rescanArchiveDirectory") })}
            {watcherState === "recovering" && status?.last_error && (
              <>
                <br />
                {t("settings.fileWatcherLastError", { error: status.last_error })}
              </>
            )}
            <br />
          </Row>
          <ActionRow
            id="open-minion"
            label={t("settings.openJobConsole")}
            onClick={() => navigate(routes.jobs())}
          >
            {t("settings.jobConsoleDescription")}
          </ActionRow>
      </div>
    </CollapsibleSection>
  )
}
