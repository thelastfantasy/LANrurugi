import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { Confirm, DateTimeStack } from "@/components/Display"
import { formatBytes } from "@/components/Display/JobProgress"
import { useHelpingImports, useUpdateHelping } from "@/share/api"
import type { HelpingImport, HelpState } from "@/share/types"
import { toast } from "@/toast"

import { ShareCodeText } from "./shared"

/** The share status class each help state borrows for its badge. */
const BADGE: Record<HelpState, string> = {
  active: "share-status-active",
  paused: "share-status-exhausted",
  sharer_offline: "share-status-expired",
  ended: "share-status-revoked",
  inactive: "",
}

/** What this instance is relaying right now. Rows disappear as soon as they stop serving, so the
 *  only action a row needs is the one that stops it. */
export function SharingHelping() {
  const { t } = useTranslation()
  const helping = useHelpingImports()
  const [stopping, setStopping] = useState<HelpingImport | null>(null)
  if (!helping.data) return null
  if (helping.data.length === 0) return <p>{t("share.helping.none")}</p>
  return (
    <>
      <div className="share-list-scroll">
        <table className="itg share-table share-helping">
          <colgroup>
            <col className="share-helping-col-title" />
            <col className="share-helping-col-code" />
            <col className="share-helping-col-status" />
            <col className="share-helping-col-uploaded" />
            <col className="share-helping-col-action" />
          </colgroup>
          <thead>
            <tr>
              <th>{t("share.form.title")}</th>
              <th>{t("share.helping.code")}</th>
              <th>{t("share.settings.status")}</th>
              <th>{t("share.helping.uploaded")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {helping.data.map((row) => (
              <HelpingRow key={row.id} row={row} onStop={() => setStopping(row)} />
            ))}
          </tbody>
        </table>
      </div>
      {stopping && <StopHelpingDialog row={stopping} onClose={() => setStopping(null)} />}
    </>
  )
}

function HelpingRow({ row, onStop }: { row: HelpingImport; onStop: () => void }) {
  const { t } = useTranslation()
  // Offline rows date from the last contact; the others from the sharer-reported change.
  const since = row.help_state === "sharer_offline" ? row.help_last_contact_at : row.help_state_at

  return (
    <tr>
      <td className="share-helping-title" title={row.title}>
        {row.title}
      </td>
      <td className="share-list-code">
        {row.code ? <ShareCodeText code={row.code} /> : "—"}
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
        <button type="button" className="favtag-btn share-helping-stop" onClick={onStop}>
          {t("share.helping.stop")}
        </button>
      </td>
    </tr>
  )
}

/** Stopping a relay is not a toggle: it drops this row out of the list and ends the uploads, so it
 *  asks first. */
function StopHelpingDialog({ row, onClose }: { row: HelpingImport; onClose: () => void }) {
  const { t } = useTranslation()
  const update = useUpdateHelping()
  return (
    <Confirm
      danger
      message={t("share.helping.stopConfirm", { title: row.title })}
      confirmLabel={t("share.helping.stop") ?? undefined}
      onCancel={onClose}
      onConfirm={() => {
        update.mutate(
          { id: row.id, help_deliver: false },
          {
            onSuccess: () => {
              toast({ heading: t("share.helping.stopped", { title: row.title }), icon: "success" })
              onClose()
            },
            onError: (e: unknown) =>
              toast({
                heading: t("share.settings.saveFailed"),
                text: e instanceof ApiError ? e.message : String(e),
                icon: "error",
              }),
          },
        )
      }}
    />
  )
}
