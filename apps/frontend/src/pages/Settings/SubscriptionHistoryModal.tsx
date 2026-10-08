import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  useBlockedWorks,
  useBlockSubscriptionSources,
  useForgetSeen,
  useStartQueueItem,
  useSubscriptionHistory,
} from "@/api/hooks"
import type { CandidateRecord, HistoryEntry } from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { TagTable } from "@/components/Display/TagTable"
import { useLanguageOrder } from "@/i18n/useLanguageOrder"
import { ICON_BUTTON_STYLE } from "@/pages/Upload/shared"
import { toast } from "@/toast"

import { CandidateTitleLink } from "./CandidateTitleLink"

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
function stateLabel(state: string, t: (k: string) => string | null) {
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
      return <>{t("subscriptions.verdictTooSoon")}</>
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

/** Stand-in rows while the first history page loads.
 *
 * The same grid and column headers as the real list with bars where the rows will be, so the modal
 * does not open as a one-line "loading…" and then jump to a 65vh table. */
function HistorySkeleton() {
  const { t } = useTranslation()
  return (
    <div className="preview-grid history-grid sub-modal-preview" role="status" aria-label={t("common.loading") ?? "Loading"}>
      <div className="preview-row preview-head" aria-hidden="true">
        <span className="preview-title">{t("subscriptions.colWork")}</span>
        <span>{t("subscriptions.colSubscription")}</span>
        <span>{t("subscriptions.colWhen")}</span>
        <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
      </div>
      {Array.from({ length: 12 }, (_, row) => (
        <div className="preview-row subscriptions-skeleton-row" key={row} aria-hidden="true">
          <span className="preview-title">
            <span className="skeleton-bar" style={{ display: "block", width: `${50 + ((row * 11) % 40)}%` }} />
            <span className="skeleton-bar" style={{ display: "block", width: `${40 + ((row * 7) % 30)}%`, marginTop: 4 }} />
          </span>
          <span>
            <span className="skeleton-bar" style={{ display: "block", width: "6em" }} />
          </span>
          <span>
            <span className="skeleton-bar" style={{ display: "block", width: "7em" }} />
          </span>
          <span className="preview-verdict">
            <span className="skeleton-bar" style={{ display: "block", width: "5em", marginLeft: "auto" }} />
          </span>
        </div>
      ))}
    </div>
  )
}

/** What every subscription has seen lately, newest first, with both block directions offered on each
 *  row — see the actions in the verdict cell below.
 *
 * Interleaved rather than grouped by subscription: the question this answers is "what arrived, and
 * why did something expected not" — which is asked about the library as a whole, not about one rule.
 * Per-subscription history still exists for the other question. */
export function SubscriptionHistoryModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const history = useSubscriptionHistory()
  const blocked = useBlockedWorks()
  const forget = useForgetSeen()
  const block = useBlockSubscriptionSources()
  const titleOrder = useLanguageOrder()
  const [band, setBand] = useState<"all" | "match" | "blocked" | "failed">("all")
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

  const rows = useMemo(() => {
    if (band === "blocked") return blockedRows
    const all = history.data ?? []
    if (band === "all") return all
    if (band === "failed") return all.filter((e) => e.download?.can_retry)
    // "Accepted" is about what happened, not about what the cycle's verdict says *now*: a work that
    // was queued and has since been catalogued reads `already_held`, and one still sitting in the
    // queue reads `queued` — both were accepted, and the queue record is the evidence of it. Without
    // the `download` half this tab is empty whenever nothing happens to be in flight at this second.
    if (band === "match") return all.filter((e) => bandOf(e.candidate) === "match" || e.download != null)
    return all.filter((e) => bandOf(e.candidate) === "excluded")
  }, [history.data, band, blockedRows])

  return (
    <Modal onClose={onClose} width={980} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 10px", textAlign: "center" }}>
        {t("subscriptions.historyTitle")}
      </h3>

      {/* Filtered rather than paginated: the two useful questions are "what came in" and "what was
          turned away", and both are a filter over the same stream. */}
      <div className="sub-modal-tabs" role="tablist">
        {(["all", "match", "blocked", "failed"] as const).map((b) => (
          <button
            key={b}
            type="button"
            role="tab"
            aria-selected={band === b}
            className={band === b ? "stdbtn is-active" : "stdbtn"}
            onClick={() => setBand(b)}
          >
            {t(`subscriptions.historyBand.${b}`)}
          </button>
        ))}
      </div>

      {history.isPending || (band === "blocked" && blocked.isPending) ? (
        <HistorySkeleton />
      ) : rows.length === 0 ? (
        <p className="sub-form-hint">
          {t(band === "blocked" ? "subscriptions.blockedEmpty" : "subscriptions.historyEmpty")}
        </p>
      ) : (
        <div className="preview-grid history-grid sub-modal-preview">
          <div className="preview-row preview-head">
            <span className="preview-title">{t("subscriptions.colWork")}</span>
            <span>{t("subscriptions.colSubscription")}</span>
            <span>{t("subscriptions.colWhen")}</span>
            <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
          </div>

          {rows.map((e: HistoryEntry, i) => {
            const outcome = outcomeOf(e)
            return (
            <div
              key={`${e.subscription_id}-${e.candidate.source_url}-${e.checked_at}-${i}`}
              // A blocked work is a "no" the user gave, so it reads red whatever the work's own
              // verdict happens to be now (it is almost always `already_held`, which would paint it
              // blue — the colour of something in the library, not of something refused).
              className={`preview-row ${
                band === "blocked" ? ROW_CLASS.excluded : ROW_CLASS[outcomeOf(e)?.band ?? bandOf(e.candidate)]
              }`}
            >
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
              <span className="history-subscription">{e.subscription_name}</span>
              <span className="history-when">
                {new Date(e.checked_at * 1000).toLocaleString()}
              </span>
              <span className="preview-verdict">
                {band === "blocked" ? (
                    <>{t("subscriptions.blockedWork")}</>
                  ) : outcome ? (
                    <>{t(outcome.key)}</>
                  ) : (
                    <Verdict record={e.candidate} />
                  )}
                {/* Both directions from the row itself. The history cannot mark a blocked work by
                    itself — in it, a blocked work reads exactly like any other seen one — so the
                    block list is joined in and the row offers whichever action still applies. */}
                <span className="preview-row-actions">
                  {e.download?.can_retry && (
                    <input
                      type="button"
                      className="stdbtn"
                      disabled={retry.isPending}
                      value={t("subscriptions.retryDownload") ?? undefined}
                      onClick={() => {
                        void retry.mutateAsync(e.download!.id).then(() => {
                          toast({
                            text: t("subscriptions.retryStarted") ?? undefined,
                            icon: "info",
                          })
                        })
                      }}
                    />
                  )}
                  {isBlocked(e) ? (
                    <>
                      {band !== "blocked" && <>{t("subscriptions.blockedWork")} · </>}
                      <input
                        type="button"
                        className="stdbtn"
                        disabled={forget.isPending}
                        value={t("subscriptions.reconsider") ?? undefined}
                        onClick={() =>
                          void forget.mutateAsync({
                            id: e.subscription_id,
                            sources: [e.candidate.source_url],
                          })
                        }
                      />
                    </>
                  ) : (
                    <Tooltip
                      label={t("subscriptions.blockWork") ?? ""}
                      // Above `Modal`'s own 9001, or the portalled bubble renders behind this dialog.
                      zIndex={9700}
                    >
                      <button
                        type="button"
                        className="stdbtn"
                        style={ICON_BUTTON_STYLE}
                        aria-label={t("subscriptions.blockWork") ?? "Never download this work"}
                        disabled={block.isPending}
                        onClick={() =>
                          void block.mutateAsync({
                            id: e.subscription_id,
                            sources: [e.candidate.source_url],
                          })
                        }
                      >
                        <i className="fa fa-ban" aria-hidden="true"></i>
                      </button>
                    </Tooltip>
                  )}
                </span>
              </span>
            </div>
            )
          })}
        </div>
      )}
    </Modal>
  )
}
