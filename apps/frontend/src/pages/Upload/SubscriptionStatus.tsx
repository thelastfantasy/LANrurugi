import { type ReactNode, useState } from "react"
import { useTranslation } from "react-i18next"

import { usePendingApprovals, useSubscriptionFailedCount, useSubscriptions } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { FONT_SIZE_XS } from "@/theme"

import { type SubscriptionsBand,SubscriptionsModal } from "./SubscriptionsModal"

/** One line tying the download queue to the subscriptions that feed it.
 *
 * The two halves of "automatic downloading" live on different pages — the rules and the approval
 * list in Settings, everything they produce on this page — and they are the same workflow. Someone
 * looking at a queue that filled itself has no way to reach the rule that did it, and someone
 * editing a rule has no way to see what it produced. This is the queue's side of that link; the
 * subscriptions page links back.
 *
 * Renders nothing at all when no subscription exists: a feature nobody has turned on should not
 * take up a line on the upload page. */
export function SubscriptionStatus({ leading }: { leading?: ReactNode } = {}) {
  const { t } = useTranslation()
  /** Which tab the subscriptions dialog is open on, or `null` while it is closed. Each count below
   *  opens the tab that answers it. */
  const [openBand, setOpenBand] = useState<SubscriptionsBand | null>(null)
  const subscriptions = useSubscriptions()
  const pending = usePendingApprovals()
  const failed = useSubscriptionFailedCount()

  const all = subscriptions.data ?? []
  if (all.length === 0 && !leading) return null
  // One height for the whole row: the text pill below sizes itself from its own line box, so icon
  // controls pinned to a different height read as misaligned next to it.
  const ROW_BUTTON_SIZE = 26
  const waiting = pending.data?.length ?? 0
  const pillLabel = [
    t("upload.manageSubscriptions"),
    waiting > 0 && t("upload.subscriptionsWaiting", { count: waiting }),
    failed > 0 && t("upload.subscriptionsFailed", { count: failed }),
  ]
    .filter(Boolean)
    .join(" · ")

  return (
    // The pill carries the counts and the modal the detail; the row is `leading` on the left and the
    // pill on the right (`flex-end`, plus the left item's `marginRight: auto`).
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "flex-end",
        gap: 8,
        fontSize: FONT_SIZE_XS,
        marginBottom: 6,
        // The plugin groups below are `.option-flyout` items, which carry a 10px right margin of
        // their own — without it the pill at this row's end stuck out 9px past their right border
        // (10px less the 1px `.stdbtn` itself adds), so the two right edges did not line up.
        paddingRight: 9,
      }}
    >
      {leading && (
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            marginRight: "auto",
          }}
        >
          {leading}
        </span>
      )}
      {/* One icon-and-count pill: the subscriptions, then what needs acting on — works waiting for
          approval and failed downloads — as numbers inside it. They used to be text links beside
          it, too small to read or tap on a phone. The tooltip spells the numbers out. */}
      <Tooltip label={pillLabel}>
        <button
          type="button"
          className="stdbtn pill-btn"
          style={{ height: ROW_BUTTON_SIZE }}
          aria-label={pillLabel}
          // Straight to what needs an answer: waiting works first, then failures, else the history.
          onClick={() => setOpenBand(waiting > 0 ? "pending" : failed > 0 ? "failed" : "all")}
        >
          <i className="fa fa-rss" aria-hidden="true"></i>
          <span>{all.length}</span>
          {waiting > 0 && (
            <>
              <i className="fa fa-hourglass-half" aria-hidden="true"></i>
              <span>{waiting}</span>
            </>
          )}
          {failed > 0 && (
            <>
              <i className="fa fa-exclamation-triangle" aria-hidden="true"></i>
              <span>{failed}</span>
            </>
          )}
        </button>
      </Tooltip>
      {openBand && <SubscriptionsModal initialBand={openBand} onClose={() => setOpenBand(null)} />}
    </div>
  )
}
