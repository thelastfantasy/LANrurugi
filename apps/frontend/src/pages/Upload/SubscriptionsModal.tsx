import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useDownloadQueue,
  usePendingApprovals,
  useReservations,
  useSettings,
  useStartQueueItem,
  useSubscriptions,
  useUpdateSettings,
} from "@/api/hooks"
import type { DownloadQueueItem, DownloadQueueState } from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_XS } from "@/theme"
import { toast } from "@/toast"

import { sourceHref } from "../Settings/preferredTitle"
import { useIsTruncated } from "../Settings/SubscriptionPreview"
import { PendingApprovals } from "../Settings/SubscriptionsSection"
import { ReservationGroup } from "./ReservationGroup"

/** A subscription's name, with a tooltip when the column is too narrow for it — the same treatment
 *  the preview gives a truncated uploader, so a clipped name is still readable (and copyable). */
function SubscriptionName({ name }: { name: string }) {
  const { ref, truncated } = useIsTruncated<HTMLSpanElement>()
  const cell = (
    // `display: block` on both the cell and the tooltip's own wrapper: an inline box reports a zero
    // width, which is what makes a measurement-driven tooltip oscillate.
    <span ref={ref} className="history-subscription" style={{ display: "block" }}>
      {name}
    </span>
  )
  if (!truncated) return cell
  return (
    <Tooltip label={name} zIndex={9700} wrapperStyle={{ display: "block", minWidth: 0 }}>
      {cell}
    </Tooltip>
  )
}

/** Which band a queued record is drawn in — taken / still in flight / went wrong — reusing the
 *  preview's own three bands so a colour means the same thing here as everywhere else. */
function bandOfState(state: DownloadQueueState): "match" | "waiting" | "excluded" {
  switch (state) {
    case "done":
      return "match"
    case "error":
    case "cancelled":
      return "excluded"
    default:
      return "waiting"
  }
}

const BAND_CLASS = {
  match: "preview-row-match",
  waiting: "preview-row-waiting",
  excluded: "preview-row-excluded",
} as const

/** Only the states the queue itself offers a start button for. */
function canRetry(state: DownloadQueueState): boolean {
  return state === "error" || state === "cancelled"
}

/** A queue item's own state in one word, for a list that has no room for the queue's own progress
 * bars and buttons. `needsDecision` states are called what they are — the question, not the wait. */
function stateLabel(state: DownloadQueueState, t: (k: string) => string | null) {
  switch (state) {
    case "done":
      return t("upload.done")
    case "cancelled":
      return t("upload.cancelled")
    case "error":
      return t("subscriptions.recordFailed")
    case "awaiting_revision_confirmation":
      return t("subscriptions.recordNeedsDecision")
    case "queued":
    case "waiting":
      return t("upload.waiting")
    case "starting":
      return t("upload.starting")
    default:
      return t("subscriptions.recordDownloading")
  }
}

/** What the subscriptions have matched, without leaving the queue those matches land in.
 *
 * The matched records rather than a list of the subscriptions themselves: the rules are
 * configuration, they live in Settings, and the counts above are already enough to know how many
 * there are. What this page's user cannot otherwise see is where each match ended up — still in the
 * download queue, waiting for a go-ahead, or set aside because it could not be fetched at all.
 * Those three are disjoint and together cover everything a rule matched, so the modal shows them in
 * that order. The full page is one click away, pinned to the bottom of the modal so it stays
 * reachable however long the lists get. */
export function SubscriptionsModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const subscriptions = useSubscriptions()
  const queue = useDownloadQueue()
  // Failed downloads are restartable, and the item is right here — hunting for it on the queue
  // behind the modal is exactly the trip this list exists to save.
  const retry = useStartQueueItem()
  const settings = useSettings()
  const updateSettings = useUpdateSettings()
  // Local draft so typing does not fire a request per keystroke; committed on blur. `null` means
  // "follow the server", which is what the input shows until the user touches it.
  const [retentionDraft, setRetentionDraft] = useState<number | null>(null)
  const retentionDays = retentionDraft ?? settings.data?.download_queue_retention_days ?? 30

  const pending = usePendingApprovals()
  const reservations = useReservations()

  const all = subscriptions.data ?? []
  const waiting = pending.data?.length ?? 0
  const reserved = reservations.data?.length ?? 0
  // What the rules actually fetched: everyone forgets that an auto-downloading subscription never
  // produces approvals, so "nothing waiting" reads as "nothing matched" unless the queued items are
  // shown here too.
  const queued = useMemo(
    () =>
      (queue.data ?? [])
        .filter((i) => i.subscription_id)
        .sort((a, b) => b.created_at - a.created_at),
    [queue.data],
  )
  // Queue items are never pruned on their own — a `Done` one stays until someone deletes it or
  // presses "clear completed" — so this list would otherwise grow without bound for as long as the
  // server runs. Capped for reading, not truncated in storage: the count says how many there are, and
  // the full queue (with its own delete affordances) is one modal away on the same page.
  const QUEUE_RECORDS_SHOWN = 50
  const shownQueued = queued.slice(0, QUEUE_RECORDS_SHOWN)

  const nameOf = (id: string | null | undefined) =>
    all.find((s) => s.id === id)?.name ?? t("upload.fromSubscriptionUnknown") ?? ""

  return (
    <Modal onClose={onClose} width={720} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 4px" }}>
        {t("subscriptions.title")}
      </h3>
      <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.8, margin: "0 0 10px" }}>
        {t("upload.subscriptionsSummary", {
          enabled: all.filter((s) => s.state.state === "enabled").length,
          total: all.length,
        })}
        {waiting > 0 && ` · ${t("upload.subscriptionsWaiting", { count: waiting })}`}
        {reserved > 0 && ` · ${t("upload.subscriptionsReserved", { count: reserved })}`}
      </p>

      {/* Each renders nothing when it has nothing to say, so the empty state below is the only
          thing shown when a rule has matched nothing — and nothing here costs attention when there
          are no rules at all. */}
      {queued.length > 0 && (
        <div style={{ marginBottom: 12 }}>
          <h3 className="ih" style={{ fontSize: "1.0em", margin: "0 0 6px" }}>
            {t("subscriptions.queuedHeading", { count: queued.length })}
          </h3>
          {/* The same grid, bands and per-row actions the matching history uses: these are the same
              kind of record (a matched work and what became of it), so they read the same way. */}
          <div className="preview-grid queue-records-grid">
            <div className="preview-row preview-head">
              <span className="preview-title">{t("subscriptions.colWork")}</span>
              <span>{t("subscriptions.colSubscription")}</span>
              <span className="preview-verdict">{t("subscriptions.state")}</span>
            </div>
            {shownQueued.map((item: DownloadQueueItem) => (
              <div key={item.id} className={`preview-row ${BAND_CLASS[bandOfState(item.state)]}`}>
                <span className="preview-title">
                  {/* `sourceHref`, not the raw stored URL: sources are stored without a scheme,
                      and a bare `href` reads as a path relative to this app (404). */}
                  <a href={sourceHref(item.url)} target="_blank" rel="noreferrer">
                    {item.title ?? item.url}
                  </a>
                </span>
                <SubscriptionName name={nameOf(item.subscription_id)} />
                <span className="preview-verdict">
                  {stateLabel(item.state, t)}
                  {canRetry(item.state) && (
                    <>
                      {" "}
                      <input
                        type="button"
                        className="stdbtn"
                        disabled={retry.isPending}
                        value={t("subscriptions.retryDownload") ?? undefined}
                        onClick={() => {
                          void retry.mutateAsync(item.id).then(() => {
                            toast({
                              text: t("subscriptions.retryStarted") ?? undefined,
                              icon: "info",
                            })
                          })
                        }}
                      />
                    </>
                  )}
                </span>
              </div>
            ))}
          </div>
          {queued.length > shownQueued.length && (
            <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.8, margin: "6px 0 0" }}>
              {t("subscriptions.queueRecordsCapped", {
                shown: shownQueued.length,
                total: queued.length,
              })}
            </p>
          )}
        </div>
      )}
      <PendingApprovals subscriptions={all} />
      <ReservationGroup />

      {/* The queue never expires anything by itself, and this list is where its growth is visible —
          so the retention setting belongs here rather than three pages away in Settings. Written
          straight through, since it is a single number with no draft state worth keeping. */}
      <div style={{ marginTop: 12, fontSize: FONT_SIZE_XS, opacity: 0.9 }}>
        <label style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap" }}>
          <i className="fa fa-broom" aria-hidden="true"></i>
          {t("subscriptions.queueRetentionLabel")}
          <input
            className="stdinput"
            type="number"
            min={0}
            value={retentionDays}
            disabled={updateSettings.isPending || settings.isPending}
            onChange={(e) => setRetentionDraft(Math.max(0, Number(e.target.value) || 0))}
            onBlur={() => {
              if (!settings.data || retentionDays === settings.data.download_queue_retention_days) return
              void updateSettings
                .mutateAsync({ download_queue_retention_days: retentionDays })
                .then(() => {
                  setRetentionDraft(null)
                  toast({ text: t("subscriptions.queueRetentionSaved") ?? undefined, icon: "success" })
                })
            }}
            style={{ width: 64 }}
          />
          {t("subscriptions.queueRetentionUnit")}
        </label>
        <p style={{ margin: "4px 0 0", opacity: 0.75 }}>{t("subscriptions.queueRetentionHint")}</p>
      </div>
      {waiting === 0 && reserved === 0 && queued.length === 0 && (
        <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.8 }}>
          {t("subscriptions.noMatchedRecords")}
        </p>
      )}

      {/* Sticky rather than part of the flow: a long list would otherwise push the way to the full
          page (editing rules, check history, deletion) off the bottom, which is exactly the case it
          is for. `backgroundColor: inherit` keeps whichever theme's modal colour behind it — no
          colour of its own to keep in sync across the five themes. */}
      <div
        style={{
          position: "sticky",
          bottom: 0,
          display: "flex",
          justifyContent: "flex-end",
          marginTop: 12,
          padding: "8px 0",
          borderTop: "1px solid rgba(128, 128, 128, 0.35)",
          backgroundColor: "inherit",
        }}
      >
        <a className="stdbtn btn-with-icon" href={routes.settings("subscriptions")}>
          <i className="fa fa-external-link" aria-hidden="true"></i>{" "}
          {t("subscriptions.openFullSettings")}
        </a>
      </div>
    </Modal>
  )
}
