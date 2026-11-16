import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import type { UnreadableArchiveCode } from "@/api/apiError"
import { useDeleteArchive } from "@/api/hooks"
import { confirmDialog } from "@/dialog"
import { routes } from "@/lib/routes"
import { toast } from "@/toast"

/** What the reader shows for a file that is damaged or gone, instead of a generic load error
 * (#290): what is wrong, the reader's own reason, and the way to get rid of the record. */
export function UnreadableArchivePage({
  archiveId,
  title,
  code,
  reason,
}: {
  archiveId: string
  title?: string
  code: UnreadableArchiveCode
  reason: string
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const del = useDeleteArchive()

  async function moveToRecycleBin() {
    const ok = await confirmDialog(t("settings.integrityDeleteConfirm", { title: title ?? archiveId }), true)
    if (!ok) return
    await del.mutateAsync(archiveId)
    toast({ text: t("settings.integrityDeleted") ?? undefined, icon: "success" })
    navigate(routes.library())
  }

  return (
    <div className="ido" style={{ textAlign: "center", padding: 40 }}>
      <i className="fas fa-8x fa-file-circle-exclamation" aria-hidden="true"></i>
      <h2 style={{ marginTop: 16 }}>{t(`reader.unreadable.${code}`)}</h2>
      {title && <p style={{ overflowWrap: "anywhere" }}>{title}</p>}
      <p style={{ opacity: 0.8, overflowWrap: "anywhere" }}>{t("reader.unreadable.reason", { reason })}</p>
      <p style={{ opacity: 0.8 }}>{t("reader.unreadable.hint")}</p>
      <div style={{ marginTop: 16, display: "flex", gap: 8, justifyContent: "center", flexWrap: "wrap" }}>
        <input
          type="button"
          className="stdbtn"
          disabled={del.isPending}
          value={t("settings.integrityDelete") ?? undefined}
          onClick={() => void moveToRecycleBin()}
        />
        <input
          type="button"
          className="stdbtn"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    </div>
  )
}
