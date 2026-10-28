/** Sending a note to the configured tracker (specs/018) — its own tab, since it is the one action
 * here a user takes without reading anything else first. */

import { useState } from "react"
import { useTranslation } from "react-i18next"

import { submitFeedback } from "./api"

export function FeedbackView() {
  const { t } = useTranslation()
  const [message, setMessage] = useState("")
  const [sending, setSending] = useState(false)
  const [note, setNote] = useState<string | null>(null)

  /// The server owns the limits: per-account daily allowance, burst window and duplicate check.
  const send = async () => {
    setSending(true)
    setNote(null)
    try {
      const result = await submitFeedback({ message })
      setMessage("")
      setNote(t("diagnostics.feedback.sent", { remaining: result.remaining_today }))
    } catch (error) {
      setNote(String(error))
    } finally {
      setSending(false)
    }
  }

  return (
    <div className="diag-group">
      <p className="diag-tagline">{t("diagnostics.feedback.hint")}</p>
      <textarea
        className="stdinput diag-message"
        rows={6}
        maxLength={2000}
        placeholder={t("diagnostics.feedback.placeholder")}
        value={message}
        onChange={(event) => setMessage(event.target.value)}
      />
      <div className="diag-inline">
        <button
          type="button"
          className="stdbtn"
          disabled={sending || message.trim().length === 0}
          onClick={send}
        >
          {t("diagnostics.feedback.send")}
        </button>
        <span className="diag-usage">{t("diagnostics.feedback.limit")}</span>
      </div>
      {note && <p className="diag-note">{note}</p>}
    </div>
  )
}
