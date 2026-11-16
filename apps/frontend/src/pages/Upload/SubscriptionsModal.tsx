import type { TFunction } from "i18next"
import { type CSSProperties, Fragment, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useBlockedWorks,
  useBlockSubscriptionSources,
  useDiscardReservation,
  useForgetSeen,
  usePendingApprovals,
  useReservations,
  useRetryReservation,
  useSettings,
  useStartQueueItem,
  useSubscriptionFailedCount,
  useSubscriptionHistory,
  useSubscriptions,
  useUnskipSubscriptionSources,
  useUpdateSettings,
} from "@/api/hooks"
import type { CandidateRecord, HistoryEntry, QueueError, ReservationListItem } from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { CheckboxField, Select } from "@/components/common-ui/Form"
import { NumberInput } from "@/components/common-ui/Form/NumberInput"
import { TagTable } from "@/components/Display/TagTable"
import { QueueErrorText } from "@/components/Layout"
import { useLanguageOrder } from "@/i18n/useLanguageOrder"
import { routes } from "@/lib/routes"
import { FONT_SIZE_XS } from "@/theme"
import { toast } from "@/toast"

import { CandidateTitleLink } from "../Settings/CandidateTitleLink"
import { PendingApprovals } from "../Settings/PendingApprovals"
import { SubscriptionGroupHead, useCollapsedGroups } from "../Settings/SubscriptionGroupHead"
import {
  PostedCell,
  RatingCell,
  SUBSCRIPTION_LIST_COLUMNS,
  UploaderCell,
} from "../Settings/SubscriptionPreview"
import { groupBySubscription, reservationReasonText, tooSoonText } from "../Settings/subscriptionVerdicts"
import { ICON_BUTTON_STYLE } from "./shared"

export type SubscriptionsBand = "pending" | "all" | "match" | "failed" | "blocked"
type Band = SubscriptionsBand
/** In the order a match moves through them: waiting on you, everything, taken, did not download,
 *  refused. */
const BANDS: readonly Band[] = ["pending", "all", "match", "failed", "blocked"]

/** Icons rather than colours, so the state reads in every theme. */
const STATE_ICON = { enabled: "fa-toggle-on", disabled: "fa-toggle-off", paused: "fa-circle-pause" } as const

/** A failed row: a work that never reached the queue carries its reservation, one that failed in
 *  the queue its queue item. Both are "download failed"; the reason says which. */
type Row = HistoryEntry & { reservation?: ReservationListItem }

/** Which band a candidate falls into — the same three the preview uses, so a verdict reads the same
 *  wherever it appears. */
function bandOf(record: CandidateRecord): "match" | "waiting" | "excluded" | "held" {
  switch (record.verdict.verdict) {
    case "queued":
    case "awaiting_approval":
      return "match"
    case "too_soon":
      return "waiting"
    // Already held (this work or a newer revision of it): a fact about the library, not a rejection.
    case "already_held":
    case "superseded":
      return "held"
    default:
      return "excluded"
  }
}

/** What actually became of an entry, once a download exists for it.
 *
 * The cycle records the *decision* ("queued"), which stops being the whole truth the moment the
 * download finishes: a row that reads "added to the download queue" while the archive has already
 * been catalogued is the modal contradicting the library. The outcome takes over as the label, and
 * with it the band — green while it is still in flight, blue once it is in the library like any other
 * already-held work, red if it failed. */
function outcomeOf(entry: HistoryEntry): { key: string; band: keyof typeof ROW_CLASS } | null {
  // Answered "no" from the approval list, and never queued: the cycle's "awaiting approval" is stale.
  if (entry.skipped && !entry.download) return { key: "subscriptions.outcomeSkipped", band: "excluded" }
  const state = entry.download?.state
  switch (state) {
    case "done":
      return { key: "subscriptions.outcomeCatalogued", band: "held" }
    case "error":
      return { key: "subscriptions.outcomeDownloadFailed", band: "excluded" }
    case "cancelled":
      return { key: "upload.cancelled", band: "excluded" }
    case "awaiting_revision_confirmation":
      return { key: "subscriptions.verdictAwaitingApproval", band: "waiting" }
    default:
      return null
  }
}

/** One Font Awesome class per queue state. Shape carries the state and the tooltip names it — no new
 *  colours, so nothing here needs per-theme variants. */
/** Green once the work is in the library, blue while it is still in flight (or failed — the shape
 *  and the tooltip carry that, not the colour). */
function stateIconClass(state: string): string {
  return state === "done" ? "history-state-done" : "history-state-running"
}

const DOWNLOAD_STATE_ICON: Record<string, string> = {
  done: "fa-check",
  downloading: "fa-download",
  starting: "fa-hourglass-start",
  waiting: "fa-clock",
  queued: "fa-clock",
  error: "fa-exclamation-triangle",
  cancelled: "fa-ban",
  awaiting_revision_confirmation: "fa-question-circle",
}

const ROW_CLASS = {
  match: "preview-row-match",
  waiting: "preview-row-waiting",
  excluded: "preview-row-excluded",
  held: "preview-row-held",
} as const

/** A queue item's own state in one word — the same vocabulary the subscription modal uses, so a
 *  state reads the same wherever it appears. */
function stateLabel(state: string, t: TFunction) {
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

function Verdict({ record }: { record: CandidateRecord }) {
  const { t } = useTranslation()
  const v = record.verdict
  switch (v.verdict) {
    case "queued":
      return <>{t("subscriptions.verdictQueued")}</>
    case "awaiting_approval":
      return <>{t("subscriptions.verdictAwaitingApproval")}</>
    case "too_soon":
      return <>{tooSoonText(t, v.reason)}</>
    case "rejected":
      return <>{t("subscriptions.verdictRejected", { rule: v.rule })}</>
    case "reserved":
      return <>{t("subscriptions.verdictReserved", { reason: v.reason })}</>
    case "already_held":
      return <>{t("subscriptions.verdictAlreadyHeld")}</>
    case "superseded":
      return (
        <span style={{ opacity: 0.85 }}>
          {t("subscriptions.verdictSuperseded", { source: v.newer_source })}
        </span>
      )
    default:
      return <>{t("subscriptions.verdictAlreadySeen")}</>
  }
}

/** A "keep for N days" setting in the dialog's footer (`0` keeps everything). A local draft so
 *  typing does not fire a request per keystroke; committed on blur. */
function RetentionField({
  field,
  icon,
  label,
  hint,
}: {
  field: "download_queue_retention_days" | "skipped_retention_days"
  icon: string
  label: string
  hint: string
}) {
  const { t } = useTranslation()
  const settings = useSettings()
  const updateSettings = useUpdateSettings()
  const [draft, setDraft] = useState<number | null>(null)
  const saved = settings.data?.[field] ?? 30
  const days = draft ?? saved
  return (
    <label style={{ display: "flex", alignItems: "center", gap: 6, flexWrap: "wrap", fontSize: FONT_SIZE_XS }}>
      <i className={`fa ${icon}`} aria-hidden="true"></i>
      {label}
      <NumberInput
        className="stdinput"
        min={0}
        value={days}
        disabled={updateSettings.isPending || settings.isPending}
        onValueChange={(v) => setDraft(Math.max(0, v))}
        onBlur={() => {
          if (!settings.data || days === saved) return
          void updateSettings.mutateAsync({ [field]: days }).then(() => {
            setDraft(null)
            toast({ text: t("subscriptions.queueRetentionSaved") ?? undefined, icon: "success" })
          })
        }}
        style={{ width: 64, height: 21 }}
      />
      {t("subscriptions.queueRetentionUnit")}
      <span style={{ opacity: 0.7 }} title={hint}>
        <i className="fa fa-circle-info" aria-hidden="true"></i>
      </span>
    </label>
  )
}

/** A row action as an icon button, its name in the tooltip (and for screen readers). */
function RowIconButton({
  icon,
  label,
  disabled,
  onClick,
}: {
  icon: string
  label: string
  disabled?: boolean
  onClick: () => void
}) {
  return (
    // Above `Modal`'s own 9001, or the portalled bubble renders behind this dialog.
    <Tooltip label={label} zIndex={9700}>
      <button
        type="button"
        className="stdbtn"
        style={ICON_BUTTON_STYLE}
        aria-label={label}
        disabled={disabled}
        onClick={onClick}
      >
        <i className={`fa ${icon}`} aria-hidden="true"></i>
      </button>
    </Tooltip>
  )
}

/** Stand-in rows while the first history page loads.
 *
 * The same grid and column headers as the real list with bars where the rows will be, so the modal
 * does not open as a one-line "loading…" and then jump to a 65vh table. */
function HistorySkeleton() {
  const { t } = useTranslation()
  return (
    <div
      className="preview-grid sub-modal-preview"
      style={{ "--preview-grid-columns": SUBSCRIPTION_LIST_COLUMNS } as CSSProperties}
      role="status"
      aria-label={t("common.loading") ?? "Loading"}
    >
      <div className="preview-row preview-head" aria-hidden="true">
        <span className="preview-index">#</span>
        <span className="preview-title">{t("subscriptions.colWork")}</span>
        <span className="preview-posted">{t("subscriptions.field.posted_at")}</span>
        <span className="preview-uploader">{t("subscriptions.colUploader")}</span>
        <span className="preview-rating">{t("subscriptions.field.rating")}</span>
        <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
      </div>
      {Array.from({ length: 12 }, (_, row) => (
        <div className="preview-row subscriptions-skeleton-row" key={row} aria-hidden="true">
          <span className="preview-index" />
          <span className="preview-title">
            <span className="skeleton-bar" style={{ display: "block", width: `${50 + ((row * 11) % 40)}%` }} />
            <span className="skeleton-bar" style={{ display: "block", width: `${40 + ((row * 7) % 30)}%`, marginTop: 4 }} />
          </span>
          {[0, 1, 2].map((cell) => (
            <span key={cell}>
              <span className="skeleton-bar" style={{ display: "block", width: "4em" }} />
            </span>
          ))}
          <span className="preview-verdict">
            <span className="skeleton-bar" style={{ display: "block", width: "5em", marginLeft: "auto" }} />
          </span>
        </div>
      ))}
    </div>
  )
}

/** Everything the subscriptions matched and what became of it, in one dialog: the works waiting on
 * the user's go-ahead, the full match history, what was taken (and its download state), what did not
 * download and why, and what was refused. Tabs over one stream rather than separate dialogs, since
 * each is the same kind of record at a different stage; a subscription filter applies to all of
 * them. Rules themselves are configuration and stay in Settings, one click away at the bottom. */
export function SubscriptionsModal({
  onClose,
  initialBand = "all",
  initialSubscription = "",
}: {
  onClose: () => void
  initialBand?: Band
  /** Opens filtered to one subscription; `""` shows them all. */
  initialSubscription?: string
}) {
  const { t } = useTranslation()
  const history = useSubscriptionHistory()
  const reservations = useReservations()
  const retryReservation = useRetryReservation()
  const discardReservation = useDiscardReservation()
  const blocked = useBlockedWorks()
  const forget = useForgetSeen()
  const block = useBlockSubscriptionSources()
  const unskip = useUnskipSubscriptionSources()
  const titleOrder = useLanguageOrder()
  const [band, setBand] = useState<Band>(initialBand)
  const pending = usePendingApprovals()
  const failedCount = useSubscriptionFailedCount()
  // Narrows every tab to one subscription; the grouping below already separates them otherwise.
  const [subscriptionFilter, setSubscriptionFilter] = useState(initialSubscription)
  // Each tab's list is grouped by subscription, newest first inside each group, like the approval
  // list; a fold is kept per subscription across tabs.
  const groupsState = useCollapsedGroups()
  /** "All" tab only: just the works skipped from the approval list, the one outcome no other tab
   *  collects. */
  const [skippedOnly, setSkippedOnly] = useState(false)
  const subscriptions = useSubscriptions()
  const stateText = (state: "enabled" | "disabled" | "paused") =>
    state === "enabled"
      ? t("subscriptions.enabled")
      : state === "disabled"
        ? t("subscriptions.disabled")
        : t("subscriptions.paused")
  // Failed downloads are restartable (`is_startable` covers Error and Cancelled), so offering the
  // retry here saves hunting for the item on the downloads page.
  const retry = useStartQueueItem()

  const blockedRows = useMemo<HistoryEntry[]>(() => {
    // Shaped like a history entry so the same grid and the same "reconsider" action render it: the
    // block list answers "what did I say no to", and the verdict is what it is because the user
    // said so. Titles are looked up in the history when it still carries the work (it may not, once
    // the work has aged out of the retained cycles), falling back to its source URL.
    const bySource = new Map((history.data ?? []).map((e) => [e.candidate.source_url, e.candidate]))
    return (blocked.data ?? []).map((b) => ({
      subscription_id: b.subscription_id,
      subscription_name: b.subscription_name,
      checked_at: Math.floor(b.blocked_at / 1000),
      outcome: "completed",
      candidate:
        bySource.get(b.source_url) ??
        ({
          source_url: b.source_url,
          title: undefined,
          tags: [],
          verdict: { verdict: "rejected", rule: "blocked" },
        } as HistoryEntry["candidate"]),
      download: null,
    }))
  }, [blocked.data, history.data])

  // Every tab needs to know which rows are blocked: the history itself cannot say — a blocked work
  // and a merely-handled one are both just "seen" in it — so the two lists are joined here.
  const blockedKeys = useMemo(
    () => new Set((blocked.data ?? []).map((b) => `${b.subscription_id}\u0000${b.source_url}`)),
    [blocked.data],
  )
  const isBlocked = (e: HistoryEntry) =>
    blockedKeys.has(`${e.subscription_id}\u0000${e.candidate.source_url}`)

  const unfilteredRows = useMemo<Row[]>(() => {
    if (band === "blocked") return blockedRows
    const all = history.data ?? []
    if (band === "all") return all
    if (band === "failed") {
      // Failed in the queue, and failed before reaching it: one list, since to the reader both are
      // simply "it did not download". Each row's own reason tells them apart.
      const queued = all.filter((e) => e.download?.state === "error")
      const bySource = new Map(all.map((e) => [`${e.subscription_id}\u0000${e.candidate.source_url}`, e]))
      const unqueued = (reservations.data ?? []).map((r): Row => {
        const seen = bySource.get(`${r.subscription_id}\u0000${r.source_url}`)
        return {
          subscription_id: r.subscription_id,
          subscription_name: seen?.subscription_name ?? r.subscription_id,
          checked_at: r.created_at,
          outcome: "completed",
          candidate: seen?.candidate ?? {
            source_url: r.source_url,
            title: undefined,
            tags: [],
            verdict: { verdict: "reserved", reason: r.reason },
          },
          download: null,
          reservation: r,
        }
      })
      return [...queued, ...unqueued].sort((a, b) => b.checked_at - a.checked_at)
    }
    // "Accepted" is about what happened, not about what the cycle's verdict says *now*: a work that
    // was queued and has since been catalogued reads `already_held`, and one still sitting in the
    // queue reads `queued` — both were accepted, and the queue record is the evidence of it. Without
    // the `download` half this tab is empty whenever nothing happens to be in flight at this second.
    if (band === "match")
      return all.filter((e) => (bandOf(e.candidate) === "match" && !e.skipped) || e.download != null)
    return all.filter((e) => bandOf(e.candidate) === "excluded")
  }, [history.data, band, blockedRows, reservations.data])
  const rows = unfilteredRows.filter(
    (e) =>
      (!subscriptionFilter || e.subscription_id === subscriptionFilter) &&
      (band !== "all" || !skippedOnly || (e.skipped && !e.download)),
  )
  const all = subscriptions.data ?? []
  const pendingShown = (pending.data ?? []).filter(
    (p) => !subscriptionFilter || p.subscription_id === subscriptionFilter,
  ).length
  const counts: Partial<Record<Band, number>> = { pending: pending.data?.length ?? 0, failed: failedCount }

  return (
    <Modal onClose={onClose} width={980} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 4px", textAlign: "center" }}>
        {t("subscriptions.title")}
      </h3>
      <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.8, margin: "0 0 10px", textAlign: "center" }}>
        {t("upload.subscriptionsSummary", {
          enabled: all.filter((s) => s.state.state === "enabled").length,
          total: all.length,
        })}
      </p>

      {/* Filtered rather than paginated: the two useful questions are "what came in" and "what was
          turned away", and both are a filter over the same stream. */}
      <div className="sub-modal-tabs" role="tablist">
        {BANDS.map((b) => (
          <button
            key={b}
            type="button"
            role="tab"
            aria-selected={band === b}
            className={band === b ? "stdbtn is-active" : "stdbtn"}
            onClick={() => setBand(b)}
          >
            {t(`subscriptions.historyBand.${b}`)}
            {(counts[b] ?? 0) > 0 && ` (${counts[b]})`}
          </button>
        ))}
      </div>
      <div className="sub-modal-filters">
        {band === "all" && (
          <CheckboxField checked={skippedOnly} onCheckedChange={setSkippedOnly}>
            {t("subscriptions.skippedOnly")}
          </CheckboxField>
        )}
      {(subscriptions.data?.length ?? 0) > 1 && (
        <div style={{ marginLeft: "auto" }}>
          <Select
            ariaLabel={t("subscriptions.historySubscriptionFilter")}
            value={subscriptionFilter}
            onValueChange={setSubscriptionFilter}
            // Above `Modal`'s own 9001, or the popup renders behind this dialog.
            popupZIndex={9700}
            items={[
              { value: "", label: t("subscriptions.historyAllSubscriptions") },
              ...(subscriptions.data ?? []).map((sub) => ({
                value: sub.id,
                label: (
                  // The switch state beside the name: a filter on a disabled subscription is a common
                  // "why is nothing new here" question, answered before it is asked.
                  <span className="history-sub-option">
                    <i className={`fa ${STATE_ICON[sub.state.state]}`} aria-hidden="true" />
                    <span>{sub.name}</span>
                    <span className="history-sub-state">{stateText(sub.state.state)}</span>
                  </span>
                ),
              })),
            ]}
          />
        </div>
      )}
      </div>

      {/* One height for every tab and filter, so switching between them does not resize the dialog. */}
      <div className="history-body">
      {band === "pending" ? (
        pendingShown === 0 ? (
          <p className="sub-form-hint">{t("subscriptions.pendingEmpty")}</p>
        ) : (
          <div className="sub-modal-preview">
            <PendingApprovals subscriptions={all} subscriptionId={subscriptionFilter} heading={false} />
          </div>
        )
      ) : history.isPending || (band === "blocked" && blocked.isPending) ? (
        <HistorySkeleton />
      ) : rows.length === 0 ? (
        <p className="sub-form-hint">
          {t(
            band === "blocked"
              ? "subscriptions.blockedEmpty"
              : band === "failed"
                ? "subscriptions.failedEmpty"
                : band === "match"
                  ? "subscriptions.matchEmpty"
                  : skippedOnly
                    ? "subscriptions.skippedEmpty"
                    : "subscriptions.historyEmpty",
          )}
        </p>
      ) : (
        // The edit form's preview columns, so a work reads the same here as where its rule is edited.
        <div
          className="preview-grid sub-modal-preview"
          style={{ "--preview-grid-columns": SUBSCRIPTION_LIST_COLUMNS } as CSSProperties}
        >
          <div className="preview-row preview-head">
            <span className="preview-index">#</span>
            <span className="preview-title">{t("subscriptions.colWork")}</span>
            <span className="preview-posted">{t("subscriptions.field.posted_at")}</span>
            <span className="preview-uploader">{t("subscriptions.colUploader")}</span>
            <span className="preview-rating">{t("subscriptions.field.rating")}</span>
            <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
          </div>

          {groupBySubscription(rows, (e) => e.subscription_id).map((group) => (
            <Fragment key={group.subscriptionId}>
              <SubscriptionGroupHead
                name={
                  subscriptions.data?.find((sub) => sub.id === group.subscriptionId)?.name ??
                  group.items[0]?.subscription_name ??
                  group.subscriptionId
                }
                count={group.items.length}
                collapsed={groupsState.isCollapsed(group.subscriptionId)}
                onToggle={() => groupsState.toggle(group.subscriptionId)}
              />
              {!groupsState.isCollapsed(group.subscriptionId) &&
                group.items.map((e: Row, i) => {
            const reservation = e.reservation
            const download = e.download
            const outcome = e.reservation
              ? { key: "subscriptions.outcomeDownloadFailed", band: "excluded" as const }
              : outcomeOf(e)
            const failure = e.reservation
              ? reservationReasonText(t, e.reservation.reason)
              : e.download?.error
                ? <QueueErrorText error={e.download.error as QueueError} />
                : null
            return (
            <div
              key={`${e.subscription_id}-${e.candidate.source_url}-${e.checked_at}-${i}`}
              // A blocked work is a "no" the user gave, so it reads red whatever the work's own
              // verdict happens to be now (it is almost always `already_held`, which would paint it
              // blue — the colour of something in the library, not of something refused).
              className={`preview-row ${
                band === "blocked" ? ROW_CLASS.excluded : ROW_CLASS[outcome?.band ?? bandOf(e.candidate)]
              }`}
            >
              <span className="preview-index">{i + 1}</span>
              <span className="preview-title">
                {/* The queue state leads the row, where a status marker is read first. Its slot is
                    fixed-width and always present: only downloaded works have a state, and letting
                    their titles start further right than everyone else's would be worse than the
                    icon is good. */}
                {e.download && (
                  <span className="history-title-state">
                    <Tooltip label={stateLabel(e.download.state, t)} zIndex={9700}>
                      <i
                        className={`fa ${DOWNLOAD_STATE_ICON[e.download.state] ?? "fa-circle"} history-state-icon ${stateIconClass(e.download.state)}`}
                        aria-label={stateLabel(e.download.state, t) ?? undefined}
                      ></i>
                    </Tooltip>
                  </span>
                )}
                <CandidateTitleLink record={e.candidate} order={titleOrder} />
                {(e.candidate.tags?.length ?? 0) > 0 && (
                  <Tooltip
                    label={<TagTable tags={(e.candidate.tags ?? []).join(",")} links={false} />}
                    wrapperStyle={{ display: "block", minWidth: 0 }}
                    // Above `Modal`'s own 9001, or the portaled bubble renders behind this dialog.
                    zIndex={9700}
                  >
                    <span className="tags">{(e.candidate.tags ?? []).join(", ")}</span>
                  </Tooltip>
                )}
              </span>
              <PostedCell postedAt={e.candidate.posted_at} />
              <UploaderCell name={e.candidate.uploader ?? ""} />
              <RatingCell rating={e.candidate.rating} />
              <span className="preview-verdict">
                {band === "blocked" ? (
                    <>{t("subscriptions.blockedWork")}</>
                  ) : outcome ? (
                    <>
                      {t(outcome.key)}
                      {failure && <span className="history-failure-reason">{failure}</span>}
                    </>
                  ) : (
                    <Verdict record={e.candidate} />
                  )}
                <span className="history-when" title={t("subscriptions.colWhen")}>
                  {new Date(e.checked_at * 1000).toLocaleString()}
                </span>
                {/* Both directions from the row itself. The history cannot mark a blocked work by
                    itself — in it, a blocked work reads exactly like any other seen one — so the
                    block list is joined in and the row offers whichever action still applies. */}
                {/* One line of icon buttons, each named by its tooltip — the same shape as the
                    approval list's, so every tab's row actions read alike. */}
                <span className="preview-row-actions">
                  {reservation?.retryable && (
                    <RowIconButton
                      icon="fa-rotate-right"
                      label={t("subscriptions.retryDownload")}
                      disabled={retryReservation.isPending}
                      onClick={() => {
                        void retryReservation
                          .mutateAsync(reservation.id)
                          .then(() => toast({ text: t("subscriptions.retryStarted") ?? undefined, icon: "info" }))
                          .catch((err: unknown) =>
                            toast({ text: err instanceof Error ? err.message : String(err), icon: "error" }),
                          )
                      }}
                    />
                  )}
                  {download?.can_retry && (
                    <RowIconButton
                      icon="fa-rotate-right"
                      label={t("subscriptions.retryDownload")}
                      disabled={retry.isPending}
                      onClick={() => {
                        void retry
                          .mutateAsync(download.id)
                          .then(() => toast({ text: t("subscriptions.retryStarted") ?? undefined, icon: "info" }))
                          .catch((err: unknown) =>
                            toast({ text: err instanceof Error ? err.message : String(err), icon: "error" }),
                          )
                      }}
                    />
                  )}
                  {e.skipped && !download && (
                    // A skipped work that is wanted after all: download it straight away (if that
                    // fails it lands back on the approval list), or return it there to decide later.
                    <RowIconButton
                      icon="fa-download"
                      label={t("subscriptions.approve")}
                      disabled={unskip.isPending}
                      onClick={() =>
                        void unskip
                          .mutateAsync({ id: e.subscription_id, sources: [e.candidate.source_url], download: true })
                          .then(() => toast({ text: t("subscriptions.retryStarted") ?? undefined, icon: "info" }))
                      }
                    />
                  )}
                  {e.skipped && !download && (
                    <RowIconButton
                      icon="fa-reply"
                      label={t("subscriptions.unskip")}
                      disabled={unskip.isPending}
                      onClick={() =>
                        void unskip
                          .mutateAsync({ id: e.subscription_id, sources: [e.candidate.source_url] })
                          .then(() => toast({ text: t("subscriptions.unskipped") ?? undefined, icon: "info" }))
                      }
                    />
                  )}
                  {reservation && (
                    // Never queued, so there is no queue row to delete: this is how it goes away,
                    // and later checks remember the answer.
                    <RowIconButton
                      icon="fa-xmark"
                      label={`${t("reservations.discard")}: ${t("reservations.discardIsRemembered")}`}
                      disabled={discardReservation.isPending}
                      onClick={() => void discardReservation.mutateAsync(reservation.id)}
                    />
                  )}
                  {isBlocked(e) ? (
                    <>
                      {band !== "blocked" && <span className="history-blocked-mark">{t("subscriptions.blockedWork")}</span>}
                      <RowIconButton
                        icon="fa-rotate-left"
                        label={t("subscriptions.reconsider")}
                        disabled={forget.isPending}
                        onClick={() =>
                          void forget.mutateAsync({ id: e.subscription_id, sources: [e.candidate.source_url] })
                        }
                      />
                    </>
                  ) : (
                    <RowIconButton
                      icon="fa-ban"
                      label={t("subscriptions.blockWork")}
                      disabled={block.isPending}
                      onClick={() =>
                        void block.mutateAsync({ id: e.subscription_id, sources: [e.candidate.source_url] })
                      }
                    />
                  )}
                </span>
              </span>
            </div>
            )
          })}
            </Fragment>
          ))}
        </div>
      )}
      </div>

      {/* The queue never expires anything by itself, and the accepted tab is where its growth shows —
          so the retention setting belongs here rather than three pages away in Settings. */}
      <div className="sub-modal-footer">
        <div className="sub-modal-retention">
          <RetentionField
            field="download_queue_retention_days"
            icon="fa-broom"
            label={t("subscriptions.queueRetentionLabel")}
            hint={t("subscriptions.queueRetentionHint")}
          />
          <RetentionField
            field="skipped_retention_days"
            icon="fa-forward"
            label={t("subscriptions.skippedRetentionLabel")}
            hint={t("subscriptions.skippedRetentionHint")}
          />
        </div>
        <a className="stdbtn btn-with-icon" href={routes.settings("subscriptions")}>
          <i className="fa fa-external-link" aria-hidden="true"></i> {t("subscriptions.openFullSettings")}
        </a>
      </div>
    </Modal>
  )
}
