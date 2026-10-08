import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"

import { useSubscriptionDetail } from "@/api/hooks"
import type { CandidateRecord, CheckCycle, Subscription } from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { Select } from "@/components/common-ui/Form"
import { TagTable } from "@/components/Display/TagTable"
import { useLanguageOrder } from "@/i18n/useLanguageOrder"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"

import { CandidateTitleLink } from "./CandidateTitleLink"

/** Which band a candidate falls into — the same three the preview and the cross-subscription
 *  history use, so a verdict reads the same wherever it appears. */
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

const ROW_CLASS = {
  match: "preview-row-match",
  waiting: "preview-row-waiting",
  excluded: "preview-row-excluded",
  held: "preview-row-held",
} as const

type Band = "all" | keyof typeof ROW_CLASS

function Verdict({ record }: { record: CandidateRecord }) {
  const { t } = useTranslation()
  const v = record.verdict
  switch (v.verdict) {
    case "queued":
      return <>{t("subscriptions.verdictQueued")}</>
    case "awaiting_approval":
      return <>{t("subscriptions.verdictAwaitingApproval")}</>
    case "reserved":
      return (
        <span style={{ color: "#c79121" }}>
          {t("subscriptions.verdictReserved", { reason: v.reason })}
        </span>
      )
    case "rejected":
      return (
        <span style={{ opacity: 0.8 }}>{t("subscriptions.verdictRejected", { rule: v.rule })}</span>
      )
    case "already_held":
      return <span style={{ opacity: 0.8 }}>{t("subscriptions.verdictAlreadyHeld")}</span>
    case "superseded":
      return (
        <span style={{ opacity: 0.85 }}>
          {t("subscriptions.verdictSuperseded", { source: v.newer_source })}
        </span>
      )
    // Distinct from "already seen", which it would otherwise fall through to: the two look alike but
    // mean opposite things — one will be looked at again, the other never will.
    case "too_soon":
      return <span style={{ color: "#c79121" }}>{t("subscriptions.verdictTooSoon")}</span>
    default:
      return <span style={{ opacity: 0.8 }}>{t("subscriptions.verdictAlreadySeen")}</span>
  }
}

/** A cycle's outcome as plain text, for the `<option>` that names it. */
function outcomeText(cycle: CheckCycle, t: (k: string, o?: Record<string, unknown>) => string | null) {
  const o = cycle.outcome
  if (o === "completed") return t("subscriptions.outcomeCompleted") ?? ""
  if ("failed" in o) return t("subscriptions.outcomeFailed", { reason: o.failed.reason }) ?? ""
  return t("subscriptions.outcomeInconclusive", { reason: o.inconclusive.reason }) ?? ""
}

/** One subscription's own check history, one cycle at a time.
 *
 * A modal rather than a row expanded inside the subscriptions table: one cycle lists every work the
 * listing returned — a hundred rows is ordinary — which inline pushed the rest of the list, and
 * everything under it, off the page. In here it gets its own scroll and its own filters. */
export function SubscriptionCheckHistoryModal({
  subscription,
  onClose,
}: {
  subscription: Subscription
  onClose: () => void
}) {
  const { t } = useTranslation()
  const titleOrder = useLanguageOrder()
  const detail = useSubscriptionDetail(subscription.id)
  /** Absent = the newest cycle, which is the one anyone opens this to see. */
  const [cycleId, setCycleId] = useState<string | null>(null)
  const [band, setBand] = useState<Band>("all")

  const cycles = detail.data?.cycles ?? []
  const cycle = cycles.find((c) => c.id === cycleId) ?? cycles[0]

  const counts = useMemo(() => {
    const out: Record<Band, number> = {
      all: cycle?.candidates.length ?? 0,
      match: 0,
      waiting: 0,
      excluded: 0,
      held: 0,
    }
    for (const cand of cycle?.candidates ?? []) out[bandOf(cand)]++
    return out
  }, [cycle])

  const rows = (cycle?.candidates ?? []).filter((cand) => band === "all" || bandOf(cand) === band)

  return (
    <Modal onClose={onClose} width={980} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 10px", textAlign: "center" }}>
        {t("subscriptions.viewHistory")} — {subscription.name}
      </h3>

      {detail.isPending ? (
        <p className="sub-form-hint">{t("common.loading")}</p>
      ) : cycles.length === 0 || !cycle ? (
        <p className="sub-form-hint">{t("subscriptions.noChecksYet")}</p>
      ) : (
        <>
          <div className="check-history-head">
            {/* A cycle is a whole listing: the older ones are only ever consulted to answer "did it
                used to work", so they live behind a picker rather than stacked down the page. */}
            <Select
              value={cycle.id}
              onValueChange={(v) => setCycleId(v)}
              ariaLabel={t("subscriptions.colWhen") ?? undefined}
              variant="stdbtn"
              size="sm"
              style={{ maxWidth: "24em" }}
              // This dialog's own z-index is 9001; the Select posts its popup to `body`.
              popupZIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
              items={cycles.map((c) => ({
                value: c.id,
                label: `${new Date(c.started_at * 1000).toLocaleString()} — ${outcomeText(c, t)}`,
              }))}
            />
            <span className="sub-form-hint">
              {t("subscriptions.candidatesSeen", { count: cycle.candidates_seen })}
            </span>
          </div>

          <div className="sub-modal-tabs" role="tablist">
            {(["all", "match", "waiting", "excluded"] as const).map((b) => (
              <button
                key={b}
                type="button"
                role="tab"
                aria-selected={band === b}
                className={band === b ? "stdbtn is-active" : "stdbtn"}
                onClick={() => setBand(b)}
              >
                {t(`subscriptions.historyBand.${b}`)} {counts[b]}
              </button>
            ))}
          </div>

          {rows.length === 0 ? (
            <p className="sub-form-hint">{t("subscriptions.historyEmpty")}</p>
          ) : (
            <div className="preview-grid check-history-grid">
              <div className="preview-row preview-head">
                <span className="preview-title">{t("subscriptions.colWork")}</span>
                <span>{t("subscriptions.colUploader")}</span>
                <span className="preview-verdict">{t("subscriptions.colVerdict")}</span>
              </div>

              {rows.map((cand, i) => (
                <div
                  key={`${cand.source_url}-${i}`}
                  className={`preview-row ${ROW_CLASS[bandOf(cand)]}`}
                >
                  <span className="preview-title">
                    <CandidateTitleLink record={cand} order={titleOrder} />
                    {(cand.tags?.length ?? 0) > 0 && (
                      <Tooltip
                        label={<TagTable tags={(cand.tags ?? []).join(",")} links={false} />}
                        wrapperStyle={{ display: "block", minWidth: 0 }}
                        // Above `Modal`'s own 9001, or the portalled bubble renders behind this dialog.
                        zIndex={9700}
                      >
                        <span className="tags">{(cand.tags ?? []).join(", ")}</span>
                      </Tooltip>
                    )}
                  </span>
                  <span className="check-history-uploader">{cand.uploader ?? "—"}</span>
                  <span className="preview-verdict">
                    <Verdict record={cand} />
                  </span>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </Modal>
  )
}
