import { useTranslation } from "react-i18next"

import { useCleanTempfolder, useClearNewFlags, useDiscardSearchCache, useShinobuAction } from "@/api/hooks"
import { CollapsibleSection } from "@/components/Display/CollapsibleSection"
import { FONT_SIZE_10PT } from "@/theme"

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
    <CollapsibleSection icon="fa-file-archive" title={t("Archive Files")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_10PT }}>
        <tbody>
          <ActionRow
            id="rescan-button"
            label={t("Rescan Archive Directory")}
            onClick={async () => {
              onStatus(t("Rescanning...") ?? "")
              await shinobuAction.mutateAsync("rescan")
              onStatus(t("Rescan queued.") ?? "")
            }}
          >
            {t("Click this button to trigger a rescan of the Archive Directory in case you're missing files, or some data such as total page counts.")}
          </ActionRow>
          <Row label={t("Maximum Cache Size")}>
            <input
              className="stdinput"
              style={{ width: "100%" }}
              maxLength={255}
              value={tempmaxsize}
              onChange={(e) => setTempmaxsize(Number(e.target.value))}
              type="text"
            />
            <br />
            {t("In MBs. The cache contains recently viewed pages, for faster subsequent reading.")}
            <br />
            {t("It is automatically emptied when it grows past this specified size.")} {t("The maximum value allowed is 4GB.")}
          </Row>
          <ActionRow
            id="clean-temp"
            label={t("Clear Cache")}
            onClick={async () => {
              await cleanTempfolder.mutateAsync()
              onStatus(t("Cache cleared.") ?? "")
            }}
          >
            <br />
            {t("Clear the cache manually by clicking this button.")}
          </ActionRow>
          <ActionRow
            id="reset-search-cache"
            label={t("Reset Search Cache")}
            onClick={async () => {
              await resetSearchCache.mutateAsync()
              onStatus(t("Search cache cleared.") ?? "")
            }}
          >
            {t("The last searches done in the archive index are cached for faster loads.")}
            <br />
            {t("If something went wrong with said cache, you can reset it by clicking this button.")}
          </ActionRow>
          <ActionRow
            id="clear-new-tags"
            label={t("Clear NEW flags")}
            onClick={async () => {
              await clearNewFlags.mutateAsync()
              onStatus(t("New flags cleared.") ?? "")
            }}
          >
            {t('Newly uploaded archives are marked as "new" in the index until you\'ve opened them.')}
            <br />
            {t("If you want to clear those flags, click this button.")}
          </ActionRow>
          <CheckboxRow id="replacedupe" checked={replacedupe} onChange={setReplacedupe} label={t("Replace duplicated archives")}>
            {t("If enabled, LANraragi will overwrite old archives when a newer one (with the same name) is uploaded through the Web Uploader or the Download System.")}
            <br />
            <i className="fas fa-exclamation-triangle" style={{ color: "red" }}></i>{" "}
            {t("This will delete metadata for old files when they're replaced! Use with caution.")}
          </CheckboxRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
