import { useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useCleanTempfolder,
  useClearNewFlags,
  useDiscardSearchCache,
  useIntegrityScan,
  useShinobuAction,
  useStartIntegrityScan,
  useStopIntegrityScan,
} from "@/api/hooks"
import { NumberInput } from "@/components/common-ui/Form/NumberInput"
import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { DamagedArchivesModal } from "./DamagedArchivesModal"
import { ActionRow, Row } from "./shared"

export function ArchiveFilesSection({
  tempmaxsize,
  setTempmaxsize,
  onStatus,
}: {
  tempmaxsize: number
  setTempmaxsize: (v: number) => void
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const shinobuAction = useShinobuAction()
  const cleanTempfolder = useCleanTempfolder()
  const resetSearchCache = useDiscardSearchCache()
  const clearNewFlags = useClearNewFlags()
  const integrity = useIntegrityScan()
  const startIntegrity = useStartIntegrityScan()
  const stopIntegrity = useStopIntegrityScan()
  const [damagedOpen, setDamagedOpen] = useState(false)
  const running = integrity.data?.running
  const lastRun = integrity.data?.last_run
  const damagedCount = integrity.data?.archives.length ?? 0

  return (
    <CollapsibleSection id="archive-files" icon="fa-file-archive" title={t("settings.archiveFiles")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          <ActionRow
            id="rescan-button"
            label={t("settings.rescanArchiveDirectory")}
            onClick={async () => {
              onStatus(t("duplicates.rescanning") ?? "")
              await shinobuAction.mutateAsync("rescan")
              onStatus(t("settings.rescanQueued") ?? "")
            }}
          >
            {t("settings.clickThisButtonToTrigger")}
          </ActionRow>
          <ActionRow
            id="integrity-scan"
            label={t("settings.integrityScan")}
            disabled={!!running || startIntegrity.isPending}
            onClick={async () => {
              const result = await startIntegrity.mutateAsync()
              onStatus(t(result.already_running ? "settings.integrityScanRunning" : "settings.integrityScanStarted"))
            }}
          >
            {t("settings.integrityScanDescription")}
            <br />
            {running
              ? t("settings.integrityScanProgress", { done: running.done, total: running.total })
              : lastRun
                ? t(lastRun.cancelled ? "settings.integrityLastRunStopped" : "settings.integrityLastRun", {
                    when: new Date(lastRun.finished_at * 1000).toLocaleString(),
                    checked: lastRun.checked,
                    damaged: lastRun.damaged,
                  })
                : t("settings.integrityNeverRun")}{" "}
            {running && (
              <button
                type="button"
                className="stdbtn"
                disabled={stopIntegrity.isPending}
                onClick={() => void stopIntegrity.mutateAsync()}
              >
                {t("settings.integrityScanStop")}
              </button>
            )}{" "}
            <button type="button" className="stdbtn" onClick={() => setDamagedOpen(true)}>
              {t("settings.integrityShowList", { count: damagedCount })}
            </button>
            {damagedOpen && <DamagedArchivesModal onClose={() => setDamagedOpen(false)} />}
          </ActionRow>
          <Row label={t("settings.maximumReaderCacheSize")}>
            <NumberInput
              className="stdinput"
              style={{ width: "100%" }}
              min={0}
              value={tempmaxsize}
              onValueChange={setTempmaxsize}
            />
            <br />
            {t("settings.inMbsThisLimitsThe")}
            <br />
            {t("settings.checkedEvery15MinutesThe")}
          </Row>
          <ActionRow
            id="clean-temp"
            label={t("settings.clearReaderCache")}
            onClick={async () => {
              await cleanTempfolder.mutateAsync()
              onStatus(t("settings.cacheCleared") ?? "")
            }}
          >
            <br />
            {t("settings.immediatelyEmptiesTheReaderS")}
          </ActionRow>
          <ActionRow
            id="reset-search-cache"
            label={t("settings.resetSearchCache")}
            onClick={async () => {
              await resetSearchCache.mutateAsync()
              onStatus(t("settings.searchCacheCleared") ?? "")
            }}
          >
            {t("settings.theLastSearchesDoneIn")}
            <br />
            {t("settings.ifSomethingWentWrongWith")}
          </ActionRow>
          <ActionRow
            id="clear-new-tags"
            label={t("settings.clearNewFlags")}
            onClick={async () => {
              await clearNewFlags.mutateAsync()
              onStatus(t("settings.newFlagsCleared") ?? "")
            }}
          >
            {t("settings.newlyUploadedArchivesAreMarked")}
            <br />
            {t("settings.ifYouWantToClear")}
          </ActionRow>
      </div>
    </CollapsibleSection>
  )
}
