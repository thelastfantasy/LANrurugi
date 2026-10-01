import "./LoadingIndicator.css"

import { useTranslation } from "react-i18next"

/**
 * Non-obscuring "translating…" affordance for a page whose look-ahead hasn't finished (T041,
 * FR-012).
 *
 * Corner-anchored on purpose: the requirement is that the original page stays readable while
 * waiting, which rules out the usual centred full-page spinner — that is exactly the default
 * pattern FR-012 exists to forbid.
 *
 * Positioning/sizing lives in a real stylesheet rather than inline `style`, per constitution
 * Principle VII: this is static, breakpoint-aware layout, not a value computed from props.
 */
export function TranslationLoadingIndicator() {
  const { t } = useTranslation()

  return (
    <div className="translation-loading" role="status" aria-live="polite">
      <span className="translation-loading__spinner" aria-hidden="true" />
      <span className="translation-loading__label">{t("translation.status.translating")}</span>
    </div>
  )
}

export default TranslationLoadingIndicator
