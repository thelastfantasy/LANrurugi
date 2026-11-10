import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router-dom";

import {
  useConfirmOlderRevision,
  useDeleteQueueItem,
  useFetchQueueItemMetadata,
  useForceDownloadQueueItem,
  useJobProgressOverride,
  usePauseQueueItem,
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
import {
  computeEta,
  computeRemainingSpeed,
  computeSpeed,
  formatBytes,
  formatDuration,
  JobProgressBar,
  STATE_COLOR,
} from "@/components/Display";
import { QueueErrorText } from "@/components/Layout";
import { routes } from "@/lib/routes";
import { FONT_SIZE_SM, FONT_SIZE_XS, Z_OVERLAY_TOOLTIP } from "@/theme";

import {
  ICON_BUTTON_STYLE,
  LOCAL_UPLOAD_NAMESPACE,
  TooltipIfPresent,
  TruncatedFilename,
} from "./shared";

/** Whether `ms` falls on today's own calendar day. */
function isToday(ms: number): boolean {
  const then = new Date(ms)
  const now = new Date()
  return (
    then.getFullYear() === now.getFullYear() &&
    then.getMonth() === now.getMonth() &&
    then.getDate() === now.getDate()
  )
}

/** When a download started, how long it has been running, how fast on average, and how big.
 *
 * Averaged over the whole run rather than the last poll: an instantaneous reading is already what
 * the progress bar's own rate-limit tooltip shows, and "how fast has this actually been going" is a
 * different question that only an average can answer. Every figure is optional — a local upload has
 * no job, a queued item has no bytes yet, and a finished job whose process has since restarted is
 * gone from the registry entirely. */
export function RowStats({
  item,
  job: jobProp,
  subscriptionName,
  onOpenSubscriptions,
}: {
  item: DownloadQueueItem
  job: JobRecord | undefined
  subscriptionName: string | undefined
  onOpenSubscriptions: () => void
}) {
  const { t } = useTranslation()
  // The same SSE-pushed bytes the progress bar uses: `computeSpeed` keeps one smoothing window per
  // job, so the two must not feed it polled values and SSE values alternately — that made every
  // other reading a negative delta and the displayed rate swing far wider than the transfer did.
  const override = useJobProgressOverride(jobProp?.id)
  const job =
    jobProp && override
      ? {
          ...jobProp,
          downloaded_bytes: override.downloaded_bytes,
          total_bytes: override.total_bytes ?? jobProp.total_bytes,
        }
      : jobProp
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
  // The bytes are all in, but the item is still `downloading`: the server is cataloguing the
  // archive and writing it into the library, which for a 1.4 GB file is minutes of disk work with
  // no byte counter of its own. Only an explicit `false` counts — a server that predates the field
  // omits it, and must keep showing the plain reading.
  const ingesting = item.state === "downloading" && item.transferring === false
  // Prefer the two *measured* rates: the job's transfer-window average while the job is still in
  // memory, then the value persisted onto the item when it completed. Deriving a rate here from
  // the whole job duration included every minute spent waiting for the server's first byte (a
  // 1.5 MB/s transfer read as 132 KB/s) and cannot exclude a resumed transfer's pre-existing
  // bytes, so it is only the last resort for rows that predate both fields.
  const transferred =
    job?.downloaded_bytes ??
    (endedMs != null ? job?.total_bytes ?? item.file_size ?? null : null)
  const legacyAverage =
    durationMs && durationMs > 0 && transferred != null
      ? (transferred / durationMs) * 1000
      : null
  const average =
    job?.average_bytes_per_sec ?? item.average_bytes_per_sec ?? legacyAverage
  // Kept separate for the trailing size: that one really is the whole payload.
  const bytes = job?.total_bytes ?? item.file_size ?? job?.downloaded_bytes ?? null
  // Time remaining, shown only while the transfer is actually running. It divides a large
  // remainder by the rate, so it uses the calmest estimate available: the server's cumulative
  // transfer average once it exists, else ~3 minutes of local history. The live 8s rate — the
  // number the progress bar prints — makes the estimate bounce by minutes on every burst, which is
  // exactly what this avoids; only the last ~10s switch to it, where the calm rate is too blunt for
  // the final seconds. `computeEta` then turns that estimate into the actual readout (a 1s/s
  // countdown between re-syncs, frozen during stalls, never negative).
  const measuredAverage = job?.average_bytes_per_sec ?? item.average_bytes_per_sec ?? null
  const calmRate =
    measuredAverage ??
    (job?.downloaded_bytes != null
      ? computeRemainingSpeed(job.id, job.downloaded_bytes)
      : null)
  const remainingBytes =
    bytes != null && job?.downloaded_bytes != null
      ? Math.max(0, bytes - job.downloaded_bytes)
      : null
  const calmMs =
    remainingBytes != null && calmRate != null && calmRate > 0
      ? (remainingBytes / calmRate) * 1000
      : null
  const liveRate =
    job?.downloaded_bytes != null ? computeSpeed(job.id, job.downloaded_bytes) : null
  const liveMs =
    remainingBytes != null && liveRate != null && liveRate > 0
      ? (remainingBytes / liveRate) * 1000
      : null
  const estimateMs =
    calmMs != null && calmMs <= 10_000 && liveMs != null ? liveMs : calmMs
  const remainingMs = job
    ? computeEta(
        job.id,
        running ? estimateMs : null,
        calmRate,
        running ? (job.downloaded_bytes ?? null) : null,
      )
    : null

  const parts = [
    // Today reads as "today <time>"; anything older carries its real date, so a row from two days
    // ago can never be mistaken for one that just started.
    startedMs &&
      (isToday(startedMs)
        ? t("upload.statStartedToday", { time: new Date(startedMs).toLocaleTimeString() })
        : t("upload.statStarted", { time: new Date(startedMs).toLocaleString() })),
    durationMs != null && t("upload.statElapsed", { duration: formatDuration(durationMs) }),
    average != null && t("upload.statAverage", { rate: formatBytes(average) }),
    !ingesting &&
      remainingMs != null &&
      t("upload.statRemaining", { duration: formatDuration(remainingMs) }),
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
  // See `RowStats`: the transfer is over but the item stays `downloading` while the server writes
  // the archive into the library. Only an explicit `false` counts (older servers omit the field).
  const ingesting = item.state === "downloading" && item.transferring === false;
  const update = useUpdateQueueItem();
  const fetchMetadata = useFetchQueueItemMetadata();
  const start = useStartQueueItem();
  const forceStart = useForceDownloadQueueItem();
  const sourceBlocked =
    item.error?.kind === "source_already_imported" ||
    item.error?.kind === "source_download_in_progress";
  const stop = useStopQueueItem();
  const pause = usePauseQueueItem();
  const confirmOlder = useConfirmOlderRevision();
  const del = useDeleteQueueItem();
  // Shared with the subscriptions page's own list (same query key), so a queue of a dozen rows
  // still costs one request.
  const subscriptions = useSubscriptions();
  const subscriptionName = item.subscription_id
    ? subscriptions.data?.find((s) => s.id === item.subscription_id)?.name
    : undefined;
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

  return (
    <>
      <div
        className="queue-item-row"
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
            item.state !== "cancelled" &&
            item.state !== "paused"
          }
          onChange={onToggleSelect}
        />

        <TooltipIfPresent
          preview={item.metadata_preview}
          url={item.url}
          // A smaller basis than the old 180px: on a phone the sum of the row's items just
          // exceeded the line, so the two icon buttons wrapped to a line of their own and every
          // row doubled in height for 26px of content (reported 2026-11-04). The title still
          // grows to fill a desktop row.
          wrapperStyle={{ flex: "1 1 110px", minWidth: 0 }}
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
                {ingesting ? (
                  <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.85 }}>
                    {t("upload.ingestingHint")}
                  </div>
                ) : null}
                {item.state === "waiting" ? (
                  <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                    {t("upload.waiting")}
                  </div>
                ) : job &&
                  job.state === "active" &&
                  job.downloaded_bytes == null ? (
                  // Nothing has arrived yet — either the plugin is still resolving the download
                  // URL or the server has not sent its first byte (an E-Hentai archive can take
                  // minutes to be built). A 0% bar here reads as stalled; `progress` is still 0.
                  <JobProgressBar
                    job={job}
                    pendingLabel={t(
                      item.state === "starting"
                        ? "upload.starting"
                        : "upload.waitingForServer",
                    )}
                    inlineLabel
                  />
                ) : job ? (
                  job.rate_limit_bytes_per_sec != null &&
                  job.rate_limit_bytes_per_sec > 0 ? (
                    <RateLimitedProgressBar
                      job={job}
                      pluginNamespace={item.plugin_namespace}
                    />
                  ) : (
                    <JobProgressBar
                      job={job}
                      inlineLabel
                      readingLabel={ingesting ? t("upload.ingesting") : undefined}
                    />
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
            ) : item.state === "paused" ? (
              <>
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
                <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                  {t("upload.paused")}
                </div>
                {/* The job that was cancelled by the pause still holds its last byte counts, so
                    the row can keep showing where the transfer stopped (until a server restart
                    drops the in-memory job). Only a known total gets a bar: the indeterminate
                    variant would keep animating, which reads as "still downloading". */}
                {job && job.downloaded_bytes != null && job.total_bytes != null ? (
                  <JobProgressBar job={job} inlineLabel />
                ) : null}
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
            {item.resume_support === false &&
              (item.state === "downloading" ||
                item.state === "starting" ||
                item.state === "waiting" ||
                item.state === "paused") && (
                <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                  {t("upload.noResumeSupport")}
                </div>
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
                className="queue-item-flag"
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

            {/* `overwrite_on_duplicate` is gone from the UI: since 020 an archive is addressed by
                its content id, so identical bytes are always reported as already present and a
                filename collision cannot happen. The field survives in the API for older clients. */}
          </>
        )}

        {/* Fixed width so a row whose state needs two buttons (pause + stop) does not narrow its
            card relative to one that needs only one: every download row's card then ends at the
            same x. 56px = two 26px tooltip-wrapped icon buttons plus their 4px gap — the width
            has to match the *two*-button content exactly, or a `flex-basis` below it is overridden
            by the content's minimum size and the cards drift apart again. */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "flex-end",
            gap: 4,
            flex: "0 0 56px",
          }}
        >
        {item.state === "awaiting_revision_confirmation" ? (
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
          <>
            <Tooltip label={t("upload.pause") ?? ""}>
              <button
                type="button"
                className="stdbtn"
                style={ICON_BUTTON_STYLE}
                disabled={pause.isPending}
                onClick={() => void pause.mutateAsync(item.id)}
              >
                <i className="fa fa-pause" aria-hidden="true"></i>
              </button>
            </Tooltip>
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
          </>
        ) : (
          <>
          {item.state === "error" && sourceBlocked && (
            <Tooltip label={t("upload.forceDownload") ?? ""}>
              <button
                type="button"
                className="stdbtn"
                style={ICON_BUTTON_STYLE}
                disabled={forceStart.isPending}
                onClick={() => void forceStart.mutateAsync(item.id)}
              >
                <i className="fa fa-download" aria-hidden="true"></i>
              </button>
            </Tooltip>
          )}
          <Tooltip
            label={
              (item.state === "error" || wasCancelled
                ? t("upload.retry")
                : item.state === "paused"
                  ? t("upload.resume")
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
                  item.state !== "cancelled" &&
                  item.state !== "paused") ||
                start.isPending
              }
              onClick={() => {
                void start.mutateAsync(item.id);
              }}
            >
              <i
                className={`fa ${
                  item.state === "paused"
                    ? "fa-play"
                    : item.state === "error" || wasCancelled
                      ? "fa-redo"
                      : "fa-download"
                }`}
                aria-hidden="true"
              ></i>
            </button>
          </Tooltip>
          </>
        )}
        </div>

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
    </>
  );
}
