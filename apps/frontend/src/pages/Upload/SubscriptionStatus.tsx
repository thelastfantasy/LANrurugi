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

  return (
    // The pill carries the count and the modal the detail, so the summary line that used to sit here
    // ("订阅：0/2 个启用") repeated what the button already says. What is left is only the counts
    // that need acting on, and the row collapses to just the button — still right-aligned, `flex-end`
    // plus the left item's `marginRight: auto` — when there are none.
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
            // With no counts to push them apart, this is what keeps the pills on the right.
            marginRight: waiting > 0 || failed > 0 ? undefined : "auto",
          }}
        >
          {leading}
        </span>
      )}
      {(waiting > 0 || failed > 0) && (
        <span
          style={{
            marginRight: "auto",
            display: "inline-flex",
            alignItems: "center",
            flexWrap: "wrap",
            gap: 6,
            opacity: 0.9,
          }}
        >
          {waiting > 0 && (
            // A link, not a label: the count is something to act on, and the approval list is the
            // first thing the subscriptions modal shows.
            <button
              type="button"
              onClick={() => setOpenBand("pending")}
              style={{
                color: "#c79121",
                background: "none",
                border: "none",
                padding: 0,
                font: "inherit",
                textDecoration: "underline",
                cursor: "pointer",
              }}
            >
              {t("upload.subscriptionsWaiting", { count: waiting })}
            </button>
          )}
          {failed > 0 && (
            <>
              {waiting > 0 && <span>·</span>}
              {/* Opens the history on its failed tab, where each row says why and offers a retry. */}
              <button
                type="button"
                onClick={() => setOpenBand("failed")}
                style={{
                  background: "none",
                  border: "none",
                  padding: 0,
                  font: "inherit",
                  color: "inherit",
                  textDecoration: "underline",
                  cursor: "pointer",
                }}
              >
                {t("upload.subscriptionsFailed", { count: failed })}
              </button>
            </>
          )}
        </span>
      )}
      {/* An icon-and-count pill rather than a text link: the number is what the button is *for*
          (this is how many subscriptions the queue is being fed by), and a pill reads as a control
          at a glance. The rss mark is the one the subscriptions' own settings section carries, so
          the button reads as "the subscriptions" rather than as this queue. */}
      <Tooltip label={t("upload.manageSubscriptions") ?? ""}>
        <button
          type="button"
          className="stdbtn pill-btn"
          style={{ height: ROW_BUTTON_SIZE }}
          aria-label={t("upload.manageSubscriptions") ?? "Manage subscriptions"}
          // Straight to the works waiting on an answer when there are any; otherwise the history.
          onClick={() => setOpenBand(waiting > 0 ? "pending" : "all")}
        >
          <i className="fa fa-rss" aria-hidden="true"></i>
          <span>{all.length}</span>
        </button>
      </Tooltip>
      {openBand && <SubscriptionsModal initialBand={openBand} onClose={() => setOpenBand(null)} />}
    </div>
  )
}
