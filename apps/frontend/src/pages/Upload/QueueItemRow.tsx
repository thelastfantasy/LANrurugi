import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { sendJson } from "@/api/client"
import {
  useDeleteQueueItem,
  useOverwriteQueueItem,
  useRenameQueueItem,
  useStartQueueItem,
  useStopQueueItem,
  useUpdateQueueItem,
} from "@/api/hooks"
import type { DownloadQueueItem, JobRecord, PluginInfo } from "@/api/types"
import { formatBytes, JobProgressBar, STATE_COLOR } from "@/components/Display"
import { Tooltip } from "@/components/Display"
import { QueueErrorText } from "@/components/Layout"
import { routes } from "@/lib/routes"
import { FONT_SIZE_8PT, FONT_SIZE_10PT, Z_OVERLAY_BACKDROP } from "@/theme"

import { ConflictMenu, RenamePopover } from "./FilenameTemplateEditor"
import { ICON_BUTTON_STYLE, LOCAL_UPLOAD_NAMESPACE, TooltipIfPresent, TruncatedFilename } from "./shared"

export async function fetchMetadataForItem(
  item: DownloadQueueItem,
  metadataPlugin: PluginInfo | null,
  update: ReturnType<typeof useUpdateQueueItem>,
) {
  if (!metadataPlugin || item.metadata_preview) return
  const result = await sendJson<{ success: number; data?: Record<string, unknown> }>(
    "POST",
    `/plugins/use?plugin=${encodeURIComponent(metadataPlugin.namespace)}&arg=${encodeURIComponent(item.url)}`,
  ).catch(() => null)
  const data = result?.data
  const title = typeof data?.title === "string" ? data.title : undefined
  if (data) await update.mutateAsync({ id: item.id, title, metadata_preview: data })
}

/** A `JobProgressBar` for a rate-limited download, with a hover tooltip anchored to *just the
 * speed figure itself* (via `JobProgressBar`'s own `speedTooltip` prop) showing the limit +
 * matched domain rule and a deep-link to that plugin's rate-limit settings (issue #2). Deliberately
 * not a `Tooltip` wrapped around the whole bar/row — that shadowed the title's own metadata-preview
 * tooltip (`TooltipIfPresent` above this bar), since both triggers would then overlap the same
 * area. */
function RateLimitedProgressBar({ job, pluginNamespace }: { job: JobRecord; pluginNamespace: string }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const cap = job.rate_limit_bytes_per_sec as number
  const pattern = job.rate_limit_matched_pattern
  return (
    <JobProgressBar
      job={job}
      speedTooltip={
        <div style={{ display: "flex", flexDirection: "column", gap: 4, maxWidth: 280 }}>
          <span>{t("Rate-limited to {{limit}}/s", { limit: formatBytes(cap) })}</span>
          {pattern && <span style={{ opacity: 0.85 }}>{t("Matched rule: {{pattern}}", { pattern })}</span>}
          <a
            onClick={(e) => {
              e.preventDefault()
              navigate(routes.pluginSettings(pluginNamespace))
            }}
            href={routes.pluginSettings(pluginNamespace)}
            style={{ textDecoration: "underline" }}
          >
            {t("Edit this plugin's rate-limit settings")}
          </a>
        </div>
      }
    />
  )
}

export function QueueItemRow({
  item,
  job,
  selected,
  onToggleSelect,
  metadataPlugin,
}: {
  item: DownloadQueueItem
  job: JobRecord | undefined
  selected: boolean
  onToggleSelect: () => void
  metadataPlugin: PluginInfo | null
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const update = useUpdateQueueItem()
  const start = useStartQueueItem()
  const stop = useStopQueueItem()
  const del = useDeleteQueueItem()
  const overwriteConflict = useOverwriteQueueItem()
  const renameConflict = useRenameQueueItem()
  const [conflictMenuAnchor, setConflictMenuAnchor] = useState<DOMRect | null>(null)
  const [renamePopover, setRenamePopover] = useState<{ x: number; y: number } | null>(null)
  const [fetchingMetadata, setFetchingMetadata] = useState(false)
  const archiveId = item.archive_ids?.[0] ?? (job?.result as { archive_ids?: string[] } | null)?.archive_ids?.[0]
  const wasCancelled = item.state === "cancelled"
  const isLocalUpload = item.plugin_namespace === LOCAL_UPLOAD_NAMESPACE
  const fileSize = item.file_size ?? job?.total_bytes

  async function handleFetchMetadata() {
    if (!metadataPlugin) return
    setFetchingMetadata(true)
    try {
      await fetchMetadataForItem(item, metadataPlugin, update)
    } finally {
      setFetchingMetadata(false)
    }
  }

  function fetchMetadataOnStart() {
    if (!metadataPlugin || item.metadata_preview || fetchingMetadata) return
    void handleFetchMetadata()
  }

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 4,
        padding: "4px 2px",
        borderTop: "1px solid rgba(128,128,128,0.2)",
        flexWrap: "wrap",
      }}
    >
      <input
        type="checkbox"
        checked={selected}
        disabled={item.state !== "queued" && item.state !== "error" && item.state !== "cancelled"}
        onChange={onToggleSelect}
      />

      <TooltipIfPresent preview={item.metadata_preview} url={item.url} wrapperStyle={{ flex: "1 1 180px", minWidth: 0 }}>
        <div
          style={{
            width: "100%",
            boxSizing: "border-box",
            border: "1px solid rgba(128,128,128,0.3)",
            borderRadius: 4,
            padding: "2px 6px",
          }}
        >
          {item.state === "downloading" || item.state === "starting" ? (
            <>
              <span style={{ fontSize: FONT_SIZE_10PT, wordBreak: "break-all", display: "block", ...(!item.title && { userSelect: "all" }) }} title={item.metadata_preview ? undefined : item.url}>
                {item.title ?? item.url}
              </span>
              {job ? (
                job.rate_limit_bytes_per_sec != null && job.rate_limit_bytes_per_sec > 0 ? (
                  <RateLimitedProgressBar job={job} pluginNamespace={item.plugin_namespace} />
                ) : (
                  <JobProgressBar job={job} />
                )
              ) : (
                <span style={{ fontSize: FONT_SIZE_10PT }}>{t("Starting…")}</span>
              )}
            </>
          ) : item.state === "done" ? (
            <div style={{ position: "relative", height: 18, borderRadius: 4, overflow: "hidden", background: STATE_COLOR.active }}>
              <a
                href={archiveId ? routes.reader(archiveId) : undefined}
                onClick={(e) => {
                  if (!archiveId) return
                  e.preventDefault()
                  navigate(routes.reader(archiveId))
                }}
                style={{
                  position: "absolute",
                  inset: 0,
                  display: "flex",
                  alignItems: "center",
                  padding: "0 6px",
                  fontSize: FONT_SIZE_10PT,
                  color: "#fff",
                  textShadow: "0 1px 2px rgba(0,0,0,0.6)",
                  cursor: archiveId ? "pointer" : "default",
                }}
              >
                <TruncatedFilename
                  text={item.title ?? item.url}
                  isFilename={!item.title}
                  style={{ minWidth: 0, flexShrink: 1 }}
                />
                {fileSize != null && (
                  <span style={{ marginLeft: 6, opacity: 0.85, flexShrink: 0 }}>({formatBytes(fileSize)})</span>
                )}
              </a>
            </div>
          ) : (
            <span
              style={{ fontSize: FONT_SIZE_10PT, display: "flex", ...(!item.title && { userSelect: "all" }) }}
              title={item.metadata_preview ? undefined : item.url}
            >
              <TruncatedFilename text={item.title ?? item.url} isFilename={!item.title} />
            </span>
          )}
          {item.state === "error" && item.error && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>
              <QueueErrorText error={item.error} />
            </div>
          )}
          {wasCancelled && (
            <div style={{ fontSize: FONT_SIZE_8PT, color: STATE_COLOR.failed }}>{t("Cancelled")}</div>
          )}
        </div>
      </TooltipIfPresent>

      {!isLocalUpload && (
        <>
          <Tooltip label={t("Auto Fetch Metadata") ?? ""}>
            <input
              type="checkbox"
              checked={item.auto_fetch_metadata}
              disabled={item.state !== "queued"}
              onChange={(e) => void update.mutateAsync({ id: item.id, auto_fetch_metadata: e.target.checked })}
            />
          </Tooltip>

          <Tooltip label={t("Overwrite Duplicate") ?? ""}>
            <input
              type="checkbox"
              checked={item.overwrite_on_duplicate}
              disabled={item.state !== "queued"}
              onChange={(e) => void update.mutateAsync({ id: item.id, overwrite_on_duplicate: e.target.checked })}
            />
          </Tooltip>
        </>
      )}

      {item.pending_filename_conflict ? (
        <>
          <Tooltip label={t("Resolve Conflict") ?? ""}>
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              disabled={overwriteConflict.isPending || renameConflict.isPending}
              onClick={(e) => {
                const rect = e.currentTarget.getBoundingClientRect()
                setConflictMenuAnchor((open) => (open ? null : rect))
              }}
            >
              <i className="fa fa-clone" aria-hidden="true"></i>
            </button>
          </Tooltip>
          {conflictMenuAnchor && (
            <>
              <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={() => setConflictMenuAnchor(null)} />
              <ConflictMenu
                anchor={conflictMenuAnchor}
                onOverwrite={() => {
                  setConflictMenuAnchor(null)
                  void overwriteConflict.mutateAsync(item.id)
                }}
                onRename={() => {
                  setRenamePopover({ x: conflictMenuAnchor.left, y: conflictMenuAnchor.bottom })
                  setConflictMenuAnchor(null)
                }}
              />
            </>
          )}
          {renamePopover && (
            <RenamePopover
              anchor={renamePopover}
              conflict={item.pending_filename_conflict}
              itemTitle={item.title}
              itemNamespace={item.plugin_namespace}
              pending={renameConflict.isPending}
              onCancel={() => setRenamePopover(null)}
              onConfirm={(filename) => {
                setRenamePopover(null)
                void renameConflict.mutateAsync({ id: item.id, filename })
              }}
            />
          )}
        </>
      ) : isLocalUpload ? null : item.state === "starting" || item.state === "downloading" ? (
        <Tooltip label={t("Stop") ?? ""}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={stop.isPending}
            onClick={() => void stop.mutateAsync(item.id)}
          >
            <i className="fa fa-stop" aria-hidden="true"></i>
          </button>
        </Tooltip>
      ) : (
        <Tooltip label={(item.state === "error" || wasCancelled ? t("Retry") : t("Download")) ?? ""}>
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={
              (item.state !== "queued" && item.state !== "error" && item.state !== "cancelled") ||
              start.isPending
            }
            onClick={() => {
              void start.mutateAsync(item.id)
              fetchMetadataOnStart()
            }}
          >
            <i
              className={`fa ${item.state === "error" || wasCancelled ? "fa-redo" : "fa-download"}`}
              aria-hidden="true"
            ></i>
          </button>
        </Tooltip>
      )}

      {!isLocalUpload && (
        <Tooltip
          label={
            metadataPlugin
              ? `${t("Fetch Metadata")} (${metadataPlugin.name})`
              : (t("Fetch Metadata") ?? "")
          }
        >
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={!metadataPlugin || item.state === "done" || fetchingMetadata}
            onClick={() => void handleFetchMetadata()}
          >
            <i className={`fa ${fetchingMetadata ? "fa-spinner fa-spin" : "fa-tags"}`} aria-hidden="true"></i>
          </button>
        </Tooltip>
      )}

      <Tooltip label={(item.state === "done" ? t("Remove") : t("Delete")) ?? ""}>
        <button
          type="button"
          className="stdbtn"
          style={ICON_BUTTON_STYLE}
          disabled={
            del.isPending || item.state === "starting" || item.state === "downloading"
          }
          onClick={() => void del.mutateAsync(item.id)}
        >
          <i className={`fa ${item.state === "done" ? "fa-eraser" : "fa-times"}`} aria-hidden="true"></i>
        </button>
      </Tooltip>
    </div>
  )
}
