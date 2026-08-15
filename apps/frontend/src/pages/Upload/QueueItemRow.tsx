import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import {
  useDeleteQueueItem,
  useFetchQueueItemMetadata,
  useOverwriteQueueItem,
  useRenameQueueItem,
  useStartQueueItem,
  useStopQueueItem,
  useUpdateQueueItem,
} from "@/api/hooks";
import type {
  DownloadQueueItem,
  JobRecord,
  PluginInfo,
} from "@/api/types";
import { formatBytes, JobProgressBar, STATE_COLOR } from "@/components/Display";
import { Tooltip } from "@/components/Display";
import { QueueErrorText } from "@/components/Layout";
import { routes } from "@/lib/routes";
import { FONT_SIZE_SM, FONT_SIZE_XS, Z_OVERLAY_BACKDROP } from "@/theme";
import { dismissToast, toast } from "@/toast";

import { ComparisonResultModal } from "./ComparisonResultModal";
import { ConflictMenu, RenamePopover } from "./FilenameTemplateEditor";
import {
  ICON_BUTTON_STYLE,
  LOCAL_UPLOAD_NAMESPACE,
  TooltipIfPresent,
  TruncatedFilename,
} from "./shared";
import { useCompareStream } from "./useCompareStream";

/** A `JobProgressBar` for a rate-limited download, with a hover tooltip anchored to *just the
 * speed figure itself* (via `JobProgressBar`'s own `speedTooltip` prop) showing the limit +
 * matched domain rule and a deep-link to that plugin's rate-limit settings (issue #2). Deliberately
 * not a `Tooltip` wrapped around the whole bar/row — that shadowed the title's own metadata-preview
 * tooltip (`TooltipIfPresent` above this bar), since both triggers would then overlap the same
 * area. */
function RateLimitedProgressBar({
  job,
  pluginNamespace,
}: {
  job: JobRecord;
  pluginNamespace: string;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cap = job.rate_limit_bytes_per_sec as number;
  const pattern = job.rate_limit_matched_pattern;
  return (
    <JobProgressBar
      job={job}
      speedTooltip={
        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 4,
            maxWidth: 280,
          }}
        >
          <span>
            {t("upload.ratelimitedToLimitS", { limit: formatBytes(cap) })}
          </span>
          {pattern && (
            <span style={{ opacity: 0.85 }}>
              {t("upload.matchedRulePattern", { pattern })}
            </span>
          )}
          <a
            onClick={(e) => {
              e.preventDefault();
              navigate(routes.pluginSettings(pluginNamespace));
            }}
            href={routes.pluginSettings(pluginNamespace)}
            style={{ textDecoration: "underline" }}
          >
            {t("upload.editThisPluginSRatelimit")}
          </a>
        </div>
      }
    />
  );
}

export function QueueItemRow({
  item,
  job,
  selected,
  onToggleSelect,
  metadataPlugin,
}: {
  item: DownloadQueueItem;
  job: JobRecord | undefined;
  selected: boolean;
  onToggleSelect: () => void;
  metadataPlugin: PluginInfo | null;
}) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const update = useUpdateQueueItem();
  const fetchMetadata = useFetchQueueItemMetadata();
  const start = useStartQueueItem();
  const stop = useStopQueueItem();
  const del = useDeleteQueueItem();
  const overwriteConflict = useOverwriteQueueItem();
  const renameConflict = useRenameQueueItem();
  const compareStream = useCompareStream();
  // `ConflictMenu` takes this ref directly and tracks it live via Floating UI's `autoUpdate` — a
  // measured-once `DOMRect` snapshot went stale between opens whenever the list's own polling
  // reflowed row heights in between (reported live, both mobile and desktop).
  const [conflictMenuOpen, setConflictMenuOpen] = useState(false);
  const conflictButtonRef = useRef<HTMLButtonElement | null>(null);
  const [renamePopover, setRenamePopover] = useState<{
    x: number;
    y: number;
  } | null>(null);
  // Modal visibility is driven by whether the stream has produced anything to show yet — opened
  // the moment the FIRST sample arrives (not waiting for `done`), per issue #77's own confirmed
  // streaming design: "发第一对的时候就开始显示modal，这样是一个很大的提速". Closing the modal tears
  // the stream down early via `compareStream.close()` (see `onClose` below) rather than just
  // hiding it — an abandoned comparison shouldn't keep computing in the background.
  //
  // Real state, not a ref — `showModal` below reads it during render, so flipping it to `false` on
  // close must trigger a re-render itself. An earlier ref-based version didn't: the modal stayed
  // visible until whatever OTHER unrelated re-render happened to sweep through next (this row's own
  // 1s download-queue poll), which is exactly the reported "关闭按钮点击后延迟响应" — nothing was
  // actually slow, the close was just invisible until the next incidental render.
  const [started, setStarted] = useState(false);
  // Mirrors `started`, read synchronously inside the error-handling effect below so that effect's
  // own guard doesn't need `started` in its dependency array (which would make its `setStarted(false)`
  // call a same-effect self-trigger, flagged by the `react-hooks` linter as a cascading-render risk).
  const startedRef = useRef(false);
  const pendingCompareToastRef = useRef<ReturnType<typeof toast> | null>(null);
  const archiveId =
    item.archive_ids?.[0] ??
    (job?.result as { archive_ids?: string[] } | null)?.archive_ids?.[0];
  const wasCancelled = item.state === "cancelled";
  const isLocalUpload = item.plugin_namespace === LOCAL_UPLOAD_NAMESPACE;
  const fileSize = item.file_size ?? job?.total_bytes;

  // Backend-owned now (`POST /download_queue/{id}/fetch-metadata` → the same 10-min-cache
  // path the post-download auto-fetch uses); the updated `metadata_preview` comes back over
  // the queue SSE delta, so nothing to write locally.
  function handleFetchMetadata() {
    if (!metadataPlugin) return;
    fetchMetadata.mutate(item.id);
  }

  function handleCompare() {
    setConflictMenuOpen(false);
    // The comparison itself (perceptual-hash every sampled page, banded-DP align, decode a
    // handful of pages for the sharpness pass) still takes a few seconds before the FIRST sample
    // is ready — with no feedback at all in that window, a click reads as having not registered
    // (reported live: "点击ai选项时没有skeleton或正在处理的toast"). Dismissed the moment the modal
    // actually opens (first sample) or a stream-level error arrives, below.
    const pendingToastId = toast({
      heading: t("upload.analyzing") ?? "Analyzing…",
      icon: "info",
      hideAfter: false,
      closeOnClick: false,
    });
    pendingCompareToastRef.current = pendingToastId;
    startedRef.current = true;
    setStarted(true);
    compareStream.start(item.id);
  }

  const hasFirstSample = compareStream.state.samples.some((s) => s !== undefined);
  const streamDone = compareStream.finished || compareStream.state.summary !== null;
  const noMatch = streamDone && !hasFirstSample;
  const showModal = started && hasFirstSample && !compareStream.error;

  useEffect(() => {
    if (!startedRef.current) return;
    if ((hasFirstSample || streamDone || compareStream.error) && pendingCompareToastRef.current !== null) {
      dismissToast(pendingCompareToastRef.current);
      pendingCompareToastRef.current = null;
    }
    if (noMatch) {
      toast({ heading: t("upload.noReliableComparisonResult"), icon: "info", hideAfter: false });
      startedRef.current = false;
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setStarted(false);
    }
    if (compareStream.error) {
      if (pendingCompareToastRef.current !== null) {
        dismissToast(pendingCompareToastRef.current);
        pendingCompareToastRef.current = null;
      }
      // Surfaces the server's own real error message (e.g. "the staged download no longer exists
      // on disk — try downloading again") rather than a generic string for every possible failure
      // — reported live (pre-streaming): a stale/cleaned-up staged file and a genuine bug both
      // showed the exact same undifferentiated "Comparison failed" toast, with no way to tell
      // which one it actually was.
      toast({ heading: compareStream.error, icon: "error" });
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasFirstSample, streamDone, noMatch, compareStream.error]);

  return (
    <>
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
          disabled={
            item.state !== "queued" &&
            item.state !== "error" &&
            item.state !== "cancelled"
          }
          onChange={onToggleSelect}
        />

        <TooltipIfPresent
          preview={item.metadata_preview}
          url={item.url}
          wrapperStyle={{ flex: "1 1 180px", minWidth: 0 }}
        >
          <div
            style={{
              width: "100%",
              boxSizing: "border-box",
              border: "1px solid rgba(128,128,128,0.3)",
              borderRadius: 4,
              padding: "2px 6px",
            }}
          >
            {item.state === "downloading" ||
            item.state === "starting" ||
            item.state === "waiting" ? (
              <>
                <span
                  style={{
                    fontSize: FONT_SIZE_SM,
                    wordBreak: "break-all",
                    display: "block",
                    ...(!item.title && { userSelect: "all" }),
                  }}
                  title={item.metadata_preview ? undefined : item.url}
                >
                  {item.title ?? item.url}
                </span>
                {item.state === "waiting" ? (
                  <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                    {t("upload.waiting")}
                  </div>
                ) : job ? (
                  job.rate_limit_bytes_per_sec != null &&
                  job.rate_limit_bytes_per_sec > 0 ? (
                    <RateLimitedProgressBar
                      job={job}
                      pluginNamespace={item.plugin_namespace}
                    />
                  ) : (
                    <JobProgressBar job={job} />
                  )
                ) : (
                  <span style={{ fontSize: FONT_SIZE_SM }}>
                    {t("upload.starting")}
                  </span>
                )}
              </>
            ) : item.state === "done" ? (
              <div
                style={{
                  position: "relative",
                  height: 18,
                  borderRadius: 4,
                  overflow: "hidden",
                  background: STATE_COLOR.active,
                }}
              >
                <a
                  href={archiveId ? routes.reader(archiveId) : undefined}
                  onClick={(e) => {
                    if (!archiveId) return;
                    e.preventDefault();
                    navigate(routes.reader(archiveId));
                  }}
                  style={{
                    position: "absolute",
                    inset: 0,
                    display: "flex",
                    alignItems: "center",
                    padding: "0 6px",
                    fontSize: FONT_SIZE_SM,
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
                    <span
                      style={{ marginLeft: 6, opacity: 0.85, flexShrink: 0 }}
                    >
                      ({formatBytes(fileSize)})
                    </span>
                  )}
                </a>
              </div>
            ) : (
              <span
                style={{
                  fontSize: FONT_SIZE_SM,
                  display: "flex",
                  ...(!item.title && { userSelect: "all" }),
                }}
                title={item.metadata_preview ? undefined : item.url}
              >
                <TruncatedFilename
                  text={item.title ?? item.url}
                  isFilename={!item.title}
                />
              </span>
            )}
            {item.state === "error" && item.error && (
              <div
                style={{ fontSize: FONT_SIZE_XS, color: item.error.kind === "already_patched" ? "#c79121" : STATE_COLOR.failed }}
              >
                <QueueErrorText error={item.error} />
              </div>
            )}
            {wasCancelled && (
              <div
                style={{ fontSize: FONT_SIZE_XS, color: STATE_COLOR.failed }}
              >
                {t("upload.cancelled")}
              </div>
            )}
          </div>
        </TooltipIfPresent>

        {!isLocalUpload && (
          <>
            <Tooltip label={t("upload.autoFetchMetadata") ?? ""}>
              <input
                type="checkbox"
                checked={item.auto_fetch_metadata}
                disabled={item.state !== "queued"}
                onChange={(e) =>
                  void update.mutateAsync({
                    id: item.id,
                    auto_fetch_metadata: e.target.checked,
                  })
                }
              />
            </Tooltip>

            <Tooltip label={t("upload.overwriteDuplicate") ?? ""}>
              <input
                type="checkbox"
                checked={item.overwrite_on_duplicate}
                disabled={item.state !== "queued"}
                onChange={(e) =>
                  void update.mutateAsync({
                    id: item.id,
                    overwrite_on_duplicate: e.target.checked,
                  })
                }
              />
            </Tooltip>
          </>
        )}

        {item.pending_filename_conflict ? (
          <>
            {/* `position: relative` wrapper — `ConflictMenu` renders non-portaled inside it
                (`portal={false}`) so its own `position: absolute` resolves against THIS box, not
                the viewport/document, and scrolls with the row as ordinary page content. */}
            <div style={{ position: "relative" }}>
              <Tooltip label={t("upload.resolveConflict") ?? ""}>
                <button
                  ref={conflictButtonRef}
                  type="button"
                  className="stdbtn"
                  style={ICON_BUTTON_STYLE}
                  disabled={
                    overwriteConflict.isPending || renameConflict.isPending
                  }
                  onClick={() => setConflictMenuOpen((open) => !open)}
                >
                  <i className="fa fa-clone" aria-hidden="true"></i>
                </button>
              </Tooltip>
              {conflictMenuOpen && (
                <>
                  <div
                    style={{
                      position: "fixed",
                      inset: 0,
                      zIndex: Z_OVERLAY_BACKDROP,
                    }}
                    onClick={() => setConflictMenuOpen(false)}
                  />
                  <ConflictMenu
                    onOverwrite={() => {
                      setConflictMenuOpen(false);
                      void overwriteConflict.mutateAsync({ id: item.id });
                    }}
                    onRename={() => {
                      const rect = conflictButtonRef.current?.getBoundingClientRect();
                      if (rect) setRenamePopover({ x: rect.left, y: rect.bottom });
                      setConflictMenuOpen(false);
                    }}
                    onCompare={() => void handleCompare()}
                  />
                </>
              )}
            </div>
            {renamePopover && (
              <RenamePopover
                anchor={renamePopover}
                conflict={item.pending_filename_conflict}
                itemTitle={item.title}
                itemNamespace={item.plugin_namespace}
                pending={renameConflict.isPending}
                onCancel={() => setRenamePopover(null)}
                onConfirm={(filename) => {
                  setRenamePopover(null);
                  void renameConflict.mutateAsync({ id: item.id, filename });
                }}
              />
            )}
          </>
        ) : isLocalUpload ? null : item.state === "starting" ||
          item.state === "waiting" ||
          item.state === "downloading" ? (
          <Tooltip label={t("upload.stop") ?? ""}>
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
          <Tooltip
            label={
              (item.state === "error" || wasCancelled
                ? t("upload.retry")
                : t("library.download")) ?? ""
            }
          >
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              disabled={
                (item.state !== "queued" &&
                  item.state !== "error" &&
                  item.state !== "cancelled") ||
                start.isPending
              }
              onClick={() => {
                // Metadata auto-fetch now lives backend-side (post-download, via
                // `ensure_metadata_cached`) — no frontend trigger needed on Start.
                void start.mutateAsync(item.id);
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
                ? `${t("upload.fetchMetadata")} (${metadataPlugin.name})`
                : (t("upload.fetchMetadata") ?? "")
            }
          >
            <button
              type="button"
              className="stdbtn"
              style={ICON_BUTTON_STYLE}
              disabled={!metadataPlugin || fetchMetadata.isPending}
              onClick={() => void handleFetchMetadata()}
            >
              <i
                className={`fa ${fetchMetadata.isPending ? "fa-spinner fa-spin" : "fa-tags"}`}
                aria-hidden="true"
              ></i>
            </button>
          </Tooltip>
        )}

        <Tooltip
          label={(item.state === "done" ? t("pluginOptions.remove") : t("common.delete")) ?? ""}
        >
          <button
            type="button"
            className="stdbtn"
            style={ICON_BUTTON_STYLE}
            disabled={
              del.isPending ||
              item.state === "starting" ||
              item.state === "waiting" ||
              item.state === "downloading"
            }
            onClick={() => void del.mutateAsync(item.id)}
          >
            <i
              className={`fa ${item.state === "done" ? "fa-eraser" : "fa-times"}`}
              aria-hidden="true"
            ></i>
          </button>
        </Tooltip>
      </div>
      {showModal && (
        <ComparisonResultModal
          queueItemId={item.id}
          state={compareStream.state}
          onClose={() => {
            compareStream.close();
            startedRef.current = false;
            setStarted(false);
          }}
        />
      )}
    </>
  );
}
