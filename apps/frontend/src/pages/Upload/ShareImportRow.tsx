import "@/share/share.css"

import { useQuery } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { fetchJson, sendJson } from "@/api/client"
import { useDeleteQueueItem, useStartQueueItem, useStopQueueItem } from "@/api/hooks"
import type { DownloadQueueItem, JobRecord } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { formatBytes, JobProgressBar, STATE_COLOR } from "@/components/Display/JobProgress"
import { QueueErrorText } from "@/components/Layout"
import { routes } from "@/lib/routes"
import { copyWithToast } from "@/share/clipboard"
import { ShareQr } from "@/share/ShareQr"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

interface ImportArchive {
  sharer_archive_id: string
  title: string
  size: number
  state: "pending" | "transferring" | "ingesting" | "done" | "reused" | "skipped" | "failed"
  bytes_done: number
  error: string | null
  /** Non-fatal: today, that a filename collision landed this archive under another name. */
  note?: string | null
  local_archive_id?: string | null
}

interface ImportDetail {
  id: string
  title: string
  items_done: number
  items_failed: number
  items_total: number
  bytes_done: number
  bytes_total: number
  /** Unix seconds the sharer said to come back, or 0 for an open-ended pause. */
  paused_until?: number | null
  warnings?: string[]
  archives?: ImportArchive[]
  /** The share code this import came from; only `GET /shares/imports/{id}` carries it. */
  code?: string | null
}

/** Live updates arrive over the download-queue stream (`useDownloadQueue`); this poll is only a
 * fallback for a dropped stream. */
const FALLBACK_POLL_MS = 15000

/** Same as the AI tankoubon drawer's: a book icon that opens the reader in a new tab. */
function OpenButton({ archiveId }: { archiveId: string }) {
  const { t } = useTranslation()
  return (
    <IconButton
      variant="ghost-btn"
      icon={<i className="fa fa-book-open" />}
      size={22}
      title={t("share.importRow.open")}
      onClick={() => window.open(routes.reader(archiveId), "_blank", "noreferrer")}
    />
  )
}

/// Per-archive failures arrive as `code: detail` for local write problems (disk full, read-only,
/// quota) and as plain text otherwise. The code is localized; the original stays in the tooltip.
function archiveErrorText(
  t: (key: string, options?: Record<string, unknown>) => string,
  error: string | null | undefined,
): string {
  if (!error) return ""
  const [code, detail] = error.split(/:\s*/, 2)
  return t(`share.importRow.failure.${code}`, { defaultValue: error, detail })
}

function ArchiveProgress({ archive }: { archive: ImportArchive }) {
  const ratio = archive.state === "done" || archive.state === "reused" ? 1 : archive.bytes_done / Math.max(archive.size, 1)
  return (
    <span className="share-import-archive-bar" aria-hidden="true">
      <span
        style={{
          width: `${Math.min(1, ratio) * 100}%`,
          background: STATE_COLOR.active,
        }}
      />
    </span>
  )
}

const IN_FLIGHT = new Set(["starting", "downloading", "waiting"])

/** One share import as a single queue row (017 FR-019): collapsed it is as tall as a download's
 * row; expanded it lists every archive in a bounded, scrolling area. */
export function ShareImportRow({ item, job }: { item: DownloadQueueItem; job: JobRecord | undefined }) {
  const { t } = useTranslation()
  const [expanded, setExpanded] = useState(false)
  const start = useStartQueueItem()
  const stop = useStopQueueItem()
  const del = useDeleteQueueItem()
  const importId = item.share_import_id
  const running = IN_FLIGHT.has(item.state)
  const detail = useQuery({
    queryKey: ["share-import", importId],
    queryFn: () => fetchJson<ImportDetail>(`/shares/imports/${encodeURIComponent(importId ?? "")}`),
    enabled: Boolean(importId),
    refetchInterval: running ? FALLBACK_POLL_MS : false,
  })
  const [showCode, setShowCode] = useState(false)
  const { refetch } = detail
  // Starting, finishing or failing changes the record as a whole: read it once more then.
  useEffect(() => {
    if (importId) void refetch()
  }, [item.state, importId, refetch])
  const d = detail.data
  const counts = d
    ? t("share.importRow.archives", {
        done: d.items_done,
        total: d.items_total,
      })
    : ""
  const openable = d?.archives?.filter((a) => a.local_archive_id) ?? []
  const single = d?.archives?.length === 1 ? openable[0] : undefined

  return (
    <>
      <div className="share-import-row">
        <button
          type="button"
          className="favtag-btn"
          aria-expanded={expanded}
          title={t("share.importRow.toggle")}
          onClick={() => setExpanded((v) => !v)}
        >
          <i className={`fa fa-caret-${expanded ? "down" : "right"}`} aria-hidden="true"></i>
        </button>
        <div className="share-import-row-main">
          <span style={{ fontSize: FONT_SIZE_SM }} title={item.title ?? item.url}>
            <i className="fa fa-share-nodes" aria-hidden="true"></i> {item.title ?? item.url}
            {counts && <span className="share-import-row-counts"> · {counts}</span>}
          </span>
          {running && d?.paused_until != null ? (
            <div style={{ fontSize: FONT_SIZE_XS }}>
              {d.paused_until > 0
                ? t("share.importRow.pausedUntil", {
                    time: new Date(d.paused_until * 1000).toLocaleString(),
                  })
                : t("share.importRow.paused")}
            </div>
          ) : running && job ? (
            <JobProgressBar job={job} />
          ) : item.state === "error" && item.error ? (
            <div style={{ fontSize: FONT_SIZE_XS }}>
              <QueueErrorText error={item.error} />
            </div>
          ) : d ? (
            <div style={{ fontSize: FONT_SIZE_XS }}>
              {formatBytes(d.bytes_done)} / {formatBytes(d.bytes_total)}
              {d.items_failed > 0 && ` · ${t("share.importRow.failed", { count: d.items_failed })}`}
            </div>
          ) : null}
        </div>
        <span className="share-import-row-actions">
          {single?.local_archive_id && <OpenButton archiveId={single.local_archive_id} />}
          {d?.code && (
            <button
              type="button"
              className="stdbtn share-import-btn"
              aria-expanded={showCode}
              title={t("share.importRow.showCode")}
              onClick={() => setShowCode((v) => !v)}
            >
              <i className="fa fa-qrcode" aria-hidden="true"></i>
            </button>
          )}
          {running ? (
            <button type="button" className="stdbtn share-import-btn" onClick={() => stop.mutate(item.id)}>
              {t("share.importRow.pause")}
            </button>
          ) : item.state !== "done" ? (
            <button type="button" className="stdbtn share-import-btn" onClick={() => start.mutate(item.id)}>
              {t("share.importRow.resume")}
            </button>
          ) : null}
          {!running && (
            <button
              type="button"
              className="stdbtn share-import-btn"
              title={t("share.importRow.remove")}
              aria-label={t("share.importRow.remove")}
              onClick={() => del.mutate(item.id)}
            >
              <i className="fa fa-trash" aria-hidden="true"></i>
            </button>
          )}
        </span>
      </div>
      {showCode && d?.code && (
        <div className="share-import-code">
          <ShareQr text={d.code} className="share-import-qr" />
          <code className="share-import-code-text">{d.code}</code>
          <button
            type="button"
            className="stdbtn"
            onClick={() => void copyWithToast(d.code ?? "", t("share.result.copied"), t("share.result.copyFailed"))}
          >
            {t("share.result.copy")}
          </button>
        </div>
      )}
      {expanded && d?.warnings && d.warnings.length > 0 && (
        <ul className="share-import-warnings">
          {d.warnings.map((w) => (
            <li key={w} className="share-import-archive-note">
              {w}
            </li>
          ))}
        </ul>
      )}
      {expanded && d?.archives && (
        <ul className="share-import-archives">
          {d.archives.map((a) => (
            <li key={a.sharer_archive_id}>
              <span className="share-import-archive-title" title={a.title}>
                {a.title}
              </span>
              <ArchiveProgress archive={a} />
              <span className="share-import-archive-state">
                {a.state === "transferring"
                  ? `${formatBytes(a.bytes_done)} / ${formatBytes(a.size)}`
                  : t(`share.importRow.state.${a.state}`)}
              </span>
              {a.local_archive_id && <OpenButton archiveId={a.local_archive_id} />}
              {a.note && (
                <span className="share-import-archive-note" title={a.note}>
                  {a.note}
                </span>
              )}
              {a.state === "failed" && (
                <>
                  <span className="share-import-archive-error" title={a.error ?? ""}>
                    {archiveErrorText(t, a.error)}
                  </span>
                  <button
                    type="button"
                    className="favtag-btn"
                    onClick={() =>
                      void sendJson(
                        "POST",
                        `/shares/imports/${encodeURIComponent(d.id)}/archives/${encodeURIComponent(a.sharer_archive_id)}/retry`,
                      ).then(() => detail.refetch())
                    }
                  >
                    {t("share.importRow.retry")}
                  </button>
                </>
              )}
            </li>
          ))}
        </ul>
      )}
    </>
  )
}
