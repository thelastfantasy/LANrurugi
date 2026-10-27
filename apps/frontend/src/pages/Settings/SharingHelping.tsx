import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { Switch } from "@/components/common-ui/Form"
import { DateTimeStack } from "@/components/Display"
import { formatBytes } from "@/components/Display/JobProgress"
import { useHelpingImports, useUpdateHelping } from "@/share/api"
import type { HelpingImport, HelpState } from "@/share/types"
import { toast } from "@/toast"

/** The share status class each help state borrows for its badge. */
const BADGE: Record<HelpState, string> = {
  active: "share-status-active",
  paused: "share-status-exhausted",
  sharer_offline: "share-status-expired",
  ended: "share-status-revoked",
  inactive: "",
}

/** "Helping deliver": shares this instance helps (or helped) deliver for their sharers (T097). */
export function SharingHelping() {
  const { t } = useTranslation()
  const helping = useHelpingImports()
  if (!helping.data) return null
  if (helping.data.length === 0) return <p>{t("share.helping.none")}</p>
  return (
    <div style={{ overflowX: "auto" }}>
      <table className="itg" style={{ width: "100%" }}>
        <thead>
          <tr>
            <th>{t("share.form.title")}</th>
            <th>{t("share.helping.deliver")}</th>
            <th>{t("share.settings.status")}</th>
            <th>{t("share.helping.uploaded")}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {helping.data.map((row) => (
            <HelpingRow key={row.id} row={row} />
          ))}
        </tbody>
      </table>
    </div>
  )
}

function HelpingRow({ row }: { row: HelpingImport }) {
  const { t } = useTranslation()
  const update = useUpdateHelping()
  const failed = (e: unknown) =>
    toast({ heading: t("share.settings.saveFailed"), text: e instanceof ApiError ? e.message : String(e), icon: "error" })
  const ended = row.help_state === "ended"
  // Offline rows date from the last contact; the others from the sharer-reported change.
  const since = row.help_state === "sharer_offline" ? row.help_last_contact_at : row.help_state_at

  return (
    <tr>
      <td>{row.title}</td>
      <td>
        <Switch
          checked={row.help_deliver}
          disabled={ended || update.isPending}
          onCheckedChange={(on) => update.mutate({ id: row.id, help_deliver: on }, { onError: failed })}
        />
      </td>
      <td>
        <span className={`share-badge ${BADGE[row.help_state]}`}>{t(`share.helping.state.${row.help_state}`)}</span>
        {row.help_reason && <div>{t(`share.helping.reason.${row.help_reason}`)}</div>}
        {since !== null && row.help_state !== "inactive" && <DateTimeStack epochSeconds={since} />}
        {row.help_removed_archives > 0 && (
          <div>{t("share.helping.removedArchives", { count: row.help_removed_archives })}</div>
        )}
      </td>
      <td>{formatBytes(row.help_bytes_served)}</td>
      <td>
        {ended && (
          <button
            type="button"
            className="favtag-btn"
            onClick={() => update.mutate({ id: row.id, help_hidden: true }, { onError: failed })}
          >
            {t("share.helping.remove")}
          </button>
        )}
      </td>
    </tr>
  )
}
