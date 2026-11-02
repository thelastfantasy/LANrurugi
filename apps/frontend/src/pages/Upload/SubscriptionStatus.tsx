import { useState } from "react"
import { useTranslation } from "react-i18next"

import { usePendingApprovals, useReservations, useSubscriptions } from "@/api/hooks"
import { Tooltip } from "@/components/common-ui/Display"
import { FONT_SIZE_XS } from "@/theme"

import { SubscriptionHistoryModal } from "../Settings/SubscriptionHistoryModal"
import { SubscriptionsModal } from "./SubscriptionsModal"

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
export function SubscriptionStatus() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  /** The cross-subscription match history — the same modal the settings section's own heading opens.
   *  Reachable from here because a queue that filled itself raises the question "what did it match,
   *  and why is this one here" on *this* page, not only where the rules are edited. */
  const [historyOpen, setHistoryOpen] = useState(false)
  const subscriptions = useSubscriptions()
  const pending = usePendingApprovals()
  const reservations = useReservations()

  const all = subscriptions.data ?? []
  if (all.length === 0) return null
  const waiting = pending.data?.length ?? 0
  const reserved = reservations.data?.length ?? 0

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
      {(waiting > 0 || reserved > 0) && (
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
            <span style={{ color: "#c79121" }}>
              {t("upload.subscriptionsWaiting", { count: waiting })}
            </span>
          )}
          {reserved > 0 && (
            <span>
              {waiting > 0 && "· "}
              {t("upload.subscriptionsReserved", { count: reserved })}
            </span>
          )}
        </span>
      )}
      <Tooltip label={t("subscriptions.historyTitle") ?? ""}>
        <button
          type="button"
          className="stdbtn icon-pill-btn"
          aria-label={t("subscriptions.historyTitle") ?? "Subscription match history"}
          onClick={() => setHistoryOpen(true)}
        >
          <i className="fa fa-th" aria-hidden="true"></i>
        </button>
      </Tooltip>
      {/* An icon-and-count pill rather than a text link: the number is what the button is *for*
          (this is how many subscriptions the queue is being fed by), and a pill reads as a control
          at a glance. The rss mark is the one the subscriptions' own settings section carries, so
          the button reads as "the subscriptions" rather than as this queue. */}
      <Tooltip label={t("upload.manageSubscriptions") ?? ""}>
        <button
          type="button"
          className="stdbtn pill-btn"
          aria-label={t("upload.manageSubscriptions") ?? "Manage subscriptions"}
          onClick={() => setOpen(true)}
        >
          <i className="fa fa-rss" aria-hidden="true"></i>
          <span>{all.length}</span>
        </button>
      </Tooltip>
      {open && <SubscriptionsModal onClose={() => setOpen(false)} />}
      {historyOpen && <SubscriptionHistoryModal onClose={() => setHistoryOpen(false)} />}
    </div>
  )
}
