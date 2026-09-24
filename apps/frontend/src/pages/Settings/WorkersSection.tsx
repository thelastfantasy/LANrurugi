import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useShinobuAction, useShinobuStatus } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, Row } from "./shared"

export function WorkersSection({ onStatus }: { onStatus: (status: string) => void }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const shinobuStatus = useShinobuStatus()
  const shinobuAction = useShinobuAction()

  return (
    <CollapsibleSection id="background-workers" icon="fa-satellite" title={t("settings.backgroundWorkers")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          <Row label={t("settings.shinobuStatus")}>
            {shinobuStatus.data?.is_alive ? (
              <span>
                {t("settings.theShinobuFileWatcherIs")}{" "}
                <h2 className="ih" style={{ display: "inline", color: "rgb(26, 165, 26)" }}>
                  👍 {t("settings.ok")}
                </h2>
              </span>
            ) : (
              <span>
                {t("settings.theShinobuFileWatcherIs")}{" "}
                <h2 className="ih" style={{ display: "inline", color: "rgb(207, 37, 37)" }}>
                  👹 {t("settings.kaput")}
                </h2>
              </span>
            )}{" "}
            ({t("settings.pid")} <span id="pid">{shinobuStatus.data?.pid}</span>)
            <br />
            {t("settings.thisFileWatcherIsResponsible")}
            <br />
          </Row>
          <ActionRow
            id="restart-button"
            label={t("settings.restartFileWatcher")}
            onClick={async () => {
              await shinobuAction.mutateAsync("restart")
              onStatus(t("settings.fileWatcherRestarted") ?? "")
            }}
          >
            {t("settings.ifShinobuIsDeadOr")}
          </ActionRow>
          <ActionRow
            id="open-minion"
            label={t("settings.openMinionConsole")}
            onClick={() => navigate(routes.jobs())}
          >
            {t("settings.theMinionWorkerHandlesSpare")}
            <br />
            {t("settings.theConsoleShowsCurrentlyRunning")}
          </ActionRow>
      </div>
    </CollapsibleSection>
  )
}
