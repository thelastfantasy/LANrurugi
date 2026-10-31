import type { CSSProperties } from "react"
import { Fragment } from "react"
import { useTranslation } from "react-i18next"

import type { ImportLegacyResult } from "@/api/types"
import { FONT_SIZE_SM } from "@/theme"

/** Counts from a finished LANraragi backup import, shared by the Backup page and the first-run
 *  wizard so both list the same rows and their numbers add up the same way. */
export function ImportLegacySummary({
  result,
  style,
}: {
  result: ImportLegacyResult
  style?: CSSProperties
}) {
  const { t } = useTranslation()
  const rows: [string, number][] = [
    [t("settings.importLegacyArchivesUpdated"), result.archives_updated],
    [t("settings.importLegacyArchivesSkippedExisting"), result.archives_skipped_already_exists],
    [t("settings.importLegacyArchivesSkippedNoMatch"), result.archives_skipped_no_match],
    [t("settings.importLegacyArchivesAmbiguous"), result.archives_ambiguous_match],
    [t("settings.importLegacyTitlesMojibakeRepaired"), result.titles_mojibake_repaired],
    [t("settings.importLegacyCategoriesRestored"), result.categories_restored],
    [t("settings.importLegacyTankoubonsRestored"), result.tankoubons_restored],
    [t("settings.importLegacyStampsRestored"), result.stamps_restored],
  ]
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: "auto auto",
        columnGap: 8,
        rowGap: 2,
        width: "fit-content",
        fontSize: FONT_SIZE_SM,
        ...style,
      }}
    >
      {rows.map(([label, value]) => (
        <Fragment key={label}>
          <span style={{ textAlign: "right" }}>{label}</span>
          <span>{value}</span>
        </Fragment>
      ))}
    </div>
  )
}
