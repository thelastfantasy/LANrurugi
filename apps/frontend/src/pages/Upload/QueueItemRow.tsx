import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import {
  useConfirmOlderRevision,
  useDeleteQueueItem,
  useFetchQueueItemMetadata,
  useOverwriteQueueItem,
  useRenameQueueItem,
  useStartQueueItem,
  useStopQueueItem,
  useSubscriptions,
  useUpdateQueueItem,
} from "@/api/hooks";
import type {
  DownloadQueueItem,
  JobRecord,
  PluginInfo,
} from "@/api/types";
import { Tooltip } from "@/components/common-ui/Display"
;
import { formatBytes, formatDuration, JobProgressBar, STATE_COLOR } from "@/components/Display";
import { QueueErrorText } from "@/components/Layout";
import { routes } from "@/lib/routes";
import { FONT_SIZE_SM, FONT_SIZE_XS, Z_OVERLAY_BACKDROP, Z_OVERLAY_TOOLTIP } from "@/theme";
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

/** When a download started, how long it has been running, how fast on average, and how big.
 *
 * Averaged over the whole run rather than the last poll: an instantaneous reading is already what
 * the progress bar's own rate-limit tooltip shows, and "how fast has this actually been going" is a
 * different question that only an average can answer. Every figure is optional — a local upload has
 * no job, a queued item has no bytes yet, and a finished job whose process has since restarted is
 * gone from the registry entirely. */
export function RowStats({
  item,
  job,
  subscriptionName,
  onOpenSubscriptions,
}: {
  item: DownloadQueueItem
  job: JobRecord | undefined
  subscriptionName: string | undefined
  onOpenSubscriptions: () => void
}) {
  const { t } = useTranslation()
  const running = job?.state === "active"
  /** Ticked rather than read from the clock during render: a running download's "elapsed" must
   *  actually move (progress events can be a minute apart on a slow transfer, and a frozen `2:13`
   *  reads as stalled), and reading `Date.now()` while rendering is not idempotent. */
  const [now, setNow] = useState(0)
  useEffect(() => {
    if (!running) return
    // Both updates happen inside callbacks, not synchronously in the effect body: the clock is an
    // external system, so it is read *from* it rather than written into state on mount. The
    // zero-delay timeout is what makes the first reading appear immediately instead of one second
    // in, so a freshly started download does not briefly show a line missing its elapsed time.
    const tick = () => setNow(Date.now())
    const immediate = window.setTimeout(tick, 0)
    const interval = window.setInterval(tick, 1000)
    return () => {
      window.clearTimeout(immediate)
      window.clearInterval(interval)
    }
  }, [running])

  // Both stamps are epoch milliseconds, like every other timestamp on the wire here. The item's own
  // copies are the fallback for a download whose job has since been lost to a server restart — the
  // same reason the completed size is persisted onto the item rather than left on the job.
  const startedMs = job?.started_at ?? item.started_at ?? null
  const endedMs = job?.finished_at ?? item.finished_at ?? (running && now > 0 ? now : null)
  const durationMs = startedMs && endedMs && endedMs >= startedMs ? endedMs - startedMs : null
  const bytes = job?.total_bytes ?? job?.downloaded_bytes ?? item.file_size ?? null
  const average =
    durationMs && durationMs > 0 && bytes != null
      ? (bytes / durationMs) * 1000
      : null

  const parts = [
    startedMs && t("upload.statStarted", { time: new Date(startedMs).toLocaleTimeString() }),
    durationMs != null && t("upload.statElapsed", { duration: formatDuration(durationMs) }),
    average != null && t("upload.statAverage", { rate: formatBytes(average) }),
    bytes != null && formatBytes(bytes),
  ].filter(Boolean) as string[]

  if (parts.length === 0 && !item.subscription_id) return null

  return (
    <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.75, display: "flex", gap: 6, flexWrap: "wrap" }}>
      {item.subscription_id && (
        <Tooltip
          label={
            subscriptionName
              ? t("upload.fromSubscription", { name: subscriptionName })
              : t("upload.fromSubscriptionUnknown")
          }
          zIndex={Z_OVERLAY_TOOLTIP}
        >
          {/* Still the way into the rule that queued this: the label moved into the tooltip, the
              link stayed. */}
          <a
            href={routes.settings("subscriptions")}
            onClick={(e) => {
              e.preventDefault()
              onOpenSubscriptions()
            }}
            style={{ color: "inherit" }}
            aria-label={t("upload.fromSubscriptionUnknown") ?? undefined}
          >
            <i className="fa fa-rss" aria-hidden="true"></i>
          </a>
        </Tooltip>
      )}
      {parts.length > 0 && <span>{parts.join(" · ")}</span>}
    </div>
  )
}

/** A `JobProgressBar` for a rate-limited download, with a hover tooltip (just the speed figure,/** A `JobProgressBar` for a rate-limited download, with a hover tooltip (just the speed figure,
 * not the whole row, which would shadow the title's own metadata-preview tooltip). */
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
  const confirmOlder = useConfirmOlderRevision();
  const del = useDeleteQueueItem();
  const overwriteConflict = useOverwriteQueueItem();
  const renameConflict = useRenameQueueItem();
  const compareStream = useCompareStream();
  // Shared with the subscriptions page's own list (same query key), so a queue of a dozen rows
  // still costs one request.
  const subscriptions = useSubscriptions();
  const subscriptionName = item.subscription_id
    ? subscriptions.data?.find((s) => s.id === item.subscription_id)?.name
    : undefined;
  const [conflictMenuOpen, setConflictMenuOpen] = useState(false);
  const conflictButtonRef = useRef<HTMLButtonElement | null>(null);
  const [renamePopover, setRenamePopover] = useState<{
    x: number;
    y: number;
  } | null>(null);
  // Real state (not a ref) — showModal reads it during render, so closing must re-render.
  const [started, setStarted] = useState(false);
  const startedRef = useRef(false);
  const pendingCompareToastRef = useRef<ReturnType<typeof toast> | null>(null);
  const archiveId =
    item.archive_ids?.[0] ??
    (job?.result as { archive_ids?: string[] } | null)?.archive_ids?.[0];
  const wasCancelled = item.state === "cancelled";
  const newerRevisionId = item.pending_revision_confirmation?.newer_archive_id;
  const isLocalUpload = item.plugin_namespace === LOCAL_UPLOAD_NAMESPACE;
  const fileSize = item.file_size ?? job?.total_bytes;

  function handleFetchMetadata() {
    if (!metadataPlugin) return;
    fetchMetadata.mutate(item.id);
  }

  function handleCompare() {
    setConflictMenuOpen(false);
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
            ) : isLocalUpload && item.state === "queued" ? (
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
                  <TruncatedFilename
                    text={item.title ?? item.url}
                    isFilename={!item.title}
                  />
                </span>
                <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                  <div
                    style={{
                      flex: 1,
                      height: 8,
                      background: "rgba(128,128,128,0.25)",
                      borderRadius: 4,
                      overflow: "hidden",
                    }}
                  >
                    <div
                      style={{
                        width: "30%",
                        height: "100%",
                        background: STATE_COLOR.active,
                        borderRadius: 4,
                        animation: "lrr-indeterminate-bar 1.2s ease-in-out infinite",
                      }}
                    />
                  </div>
                  <span style={{ fontSize: FONT_SIZE_XS, whiteSpace: "nowrap" }}>
                    {t("upload.processing")}
                  </span>
                </div>
              </>
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
            {/* One line of numbers where the subscription used to be written out: what the row is
                doing (started at, how long, how fast, how big) is what a queue row is read for, and
                the rule behind it is one hover away on the icon. */}
            <RowStats
              item={item}
              job={job}
              subscriptionName={subscriptionName}
              onOpenSubscriptions={() => navigate(routes.settings("subscriptions"))}
            />
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
            {item.state === "awaiting_revision_confirmation" &&
              newerRevisionId && (
                <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                  {t("upload.olderRevisionWarning")}{" "}
                  <a
                    href={routes.reader(newerRevisionId)}
                    onClick={(e) => {
                      e.preventDefault()
                      navigate(routes.reader(newerRevisionId))
                    }}
                    style={{ color: "inherit" }}
                  >
                    {newerRevisionId}
                  </a>
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
                    stagedAt={item.pending_filename_conflict.staged_at}
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
        ) : item.state === "awaiting_revision_confirmation" ? (
          <>
            <Tooltip label={t("upload.downloadAnyway") ?? ""}>
              <button
                type="button"
                className="stdbtn"
                style={ICON_BUTTON_STYLE}
                disabled={confirmOlder.isPending}
                onClick={() =>
                  void confirmOlder.mutateAsync({ id: item.id, proceed: true })
                }
              >
                <i className="fa fa-download" aria-hidden="true"></i>
              </button>
            </Tooltip>
            <Tooltip label={t("upload.abandonDownload") ?? ""}>
              <button
                type="button"
                className="stdbtn"
                style={ICON_BUTTON_STYLE}
                disabled={confirmOlder.isPending}
                onClick={() =>
                  void confirmOlder.mutateAsync({ id: item.id, proceed: false })
                }
              >
                <i className="fa fa-ban" aria-hidden="true"></i>
              </button>
            </Tooltip>
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
