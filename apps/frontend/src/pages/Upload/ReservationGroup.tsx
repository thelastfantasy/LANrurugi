import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { useDiscardReservation, useReservations } from "@/api/hooks"
import type { ReservationEntry } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

import { ICON_BUTTON_STYLE } from "./shared"

/** Turns the server's stable reason key into a translated sentence.
 *
 * Reasons arrive as keys (`insufficient_credit`, `download_failed:<detail>`) rather than English
 * prose precisely so they can be translated; an unrecognised key falls back to showing itself,
 * which is more useful than an empty cell. */
function reasonText(reason: string, t: (k: string, o?: Record<string, unknown>) => string | null) {
  if (reason === "source_unreachable") return t("reservations.sourceUnreachable") ?? reason
  if (reason === "insufficient_credit") return t("reservations.insufficientCredit") ?? reason
  if (reason.startsWith("download_failed:")) {
    return t("reservations.downloadFailed", { detail: reason.slice("download_failed:".length) }) ?? reason
  }
  return reason
}

/** Matched works that could not be downloaded, with why.
 *
 * Lives on the upload page rather than in settings because it is a queue of things awaiting the
 * user's decision — the same kind of thing as the download queue beside it — not configuration.
 *
 * Collapsed by default behind its own header: when nothing has failed, this must cost no attention
 * at all. The header is only rendered when there is something to show, so an empty list is
 * genuinely invisible rather than an empty box. */
export function ReservationGroup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const reservations = useReservations()
  const discard = useDiscardReservation()
  const [open, setOpen] = useState(false)

  const entries: ReservationEntry[] = reservations.data ?? []
  if (entries.length === 0) return null

  return (
    <div style={{ marginTop: 8 }}>
      <div
        className={`collapsible-title caret-right${open ? " active" : ""}`}
        style={{ padding: "5px 0 0 5px", cursor: "pointer" }}
        onClick={() => setOpen((o) => !o)}
      >
        <i className="fas fa-bookmark fa-2x" style={{ marginRight: 4 }} aria-hidden="true"></i>
        <b style={{ verticalAlign: "super" }}>
          {t("reservations.title")} ({entries.length})
        </b>
      </div>
      {open && (
        <div className="collapsible-body" style={{ padding: "5px 0 0 0" }}>
          {entries.map((entry) => (
            <div
              key={entry.id}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 6,
                padding: "4px 2px",
                borderTop: "1px solid rgba(128,128,128,0.2)",
                flexWrap: "wrap",
              }}
            >
              <div style={{ flex: "1 1 220px", minWidth: 0 }}>
                <div style={{ fontSize: FONT_SIZE_SM, wordBreak: "break-all" }}>
                  {entry.source_url}
                </div>
                <div style={{ fontSize: FONT_SIZE_XS, color: "#c79121" }}>
                  {reasonText(entry.reason, t)}{" "}
                  {/* Reaching the subscription that produced this matters: a rule that keeps
                      generating failures is only fixable if you can find it. */}
                  <a
                    href={routes.settings("subscriptions")}
                    onClick={(e) => {
                      e.preventDefault()
                      navigate(routes.settings("subscriptions"))
                    }}
                    style={{ color: "inherit", textDecoration: "underline" }}
                  >
                    {t("reservations.fromSubscription")}
                  </a>
                </div>
              </div>
              <Tooltip label={t("reservations.discard") ?? ""}>
                <button
                  type="button"
                  className="stdbtn"
                  style={ICON_BUTTON_STYLE}
                  disabled={discard.isPending}
                  onClick={() => void discard.mutateAsync(entry.id)}
                >
                  <i className="fa fa-ban" aria-hidden="true"></i>
                </button>
              </Tooltip>
            </div>
          ))}
          <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.8, marginTop: 6 }}>
            {t("reservations.discardIsRemembered")}
          </p>
        </div>
      )}
    </div>
  )
}
