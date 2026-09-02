import { useTranslation } from "react-i18next"

import { useCleanTempfolder, useClearNewFlags, useDiscardSearchCache, useShinobuAction } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { ActionRow, CheckboxRow, Row } from "./shared"

export function ArchiveFilesSection({
  tempmaxsize,
  setTempmaxsize,
  replacedupe,
  setReplacedupe,
  onStatus,
}: {
  tempmaxsize: number
  setTempmaxsize: (v: number) => void
  replacedupe: boolean
  setReplacedupe: (v: boolean) => void
  onStatus: (status: string) => void
}) {
  const { t } = useTranslation()
  const shinobuAction = useShinobuAction()
  const cleanTempfolder = useCleanTempfolder()
  const resetSearchCache = useDiscardSearchCache()
  const clearNewFlags = useClearNewFlags()

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
          <Row label={t("settings.maximumReaderCacheSize")}>
            <input
              className="stdinput"
              style={{ width: "100%" }}
              maxLength={255}
              value={tempmaxsize}
              onChange={(e) => setTempmaxsize(Number(e.target.value))}
              type="text"
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
          <CheckboxRow id="replacedupe" checked={replacedupe} onChange={setReplacedupe} label={t("settings.replaceDuplicatedArchives")}>
            {t("settings.ifEnabledLanraragiWillOverwrite")}
            <br />
            <i className="fas fa-exclamation-triangle" style={{ color: "red" }}></i>{" "}
            {t("settings.thisWillDeleteMetadataFor")}
          </CheckboxRow>
      </div>
    </CollapsibleSection>
  )
}
