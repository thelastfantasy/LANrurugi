import { useTranslation } from "react-i18next"

/**
 * Actionable guidance when the browser can't reach the user's local model (T052, FR-018,
 * research.md §9).
 *
 * Browsers restrict a page from calling a device on the user's own network (Private Network
 * Access), and script can't distinguish that block from the backend simply being down — both
 * surface as an opaque `TypeError`. So rather than a generic "request failed", this explains the
 * likely cause and gives the cheapest real fix first: configure the local backend to accept this
 * origin, which needs no extra software installed at all.
 *
 * Deliberately absent: any suggestion to disable browser security features. The constitution
 * explicitly rules that out as a supported workaround.
 */
export function LocalBackendGuidance({
  onRetry,
  detail,
}: {
  onRetry?: () => void
  detail?: string
}) {
  const { t } = useTranslation()

  return (
    <div className="translation-local-guidance" role="alert">
      <h3>{t("translation.localBlocked.title")}</h3>
      <p>{t("translation.localBlocked.body")}</p>
      <p className="helptext">{t("translation.localBlocked.ollamaHint")}</p>

      {detail && <p className="helptext">{detail}</p>}

      <p>
        <a href="/docs/translation-local-backend.md" target="_blank" rel="noreferrer">
          {t("translation.localBlocked.docs")}
        </a>
      </p>

      {onRetry && (
        <button type="button" className="stdbtn" onClick={onRetry}>
          {t("translation.localBlocked.retry")}
        </button>
      )}
    </div>
  )
}

export default LocalBackendGuidance
