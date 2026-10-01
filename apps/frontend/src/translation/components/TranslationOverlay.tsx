import "./TranslationOverlay.css"

import { useTranslation } from "react-i18next"

import { resolveActiveBackend } from "../settings"
import type { TranslationSettings } from "../types"
import { usePageTranslation } from "../usePageTranslation"
import TranslationLoadingIndicator from "./LoadingIndicator"
import LocalBackendGuidance from "./LocalBackendGuidance"

/**
 * Reader overlay showing a page's translated rendering when one is available (T032/T057).
 *
 * Deliberately an *overlay* rather than a change to the reader's own `<img src>` chain: that chain
 * already juggles preview blobs, full blobs, and an error fallback, and threading a fourth source
 * through it would put a Phase 2 feature squarely in the path of Phase 1's core reading experience.
 * As an absolutely-positioned layer, the original page underneath renders exactly as it always
 * did — which is also precisely what FR-012/FR-019 require while a translation is pending or has
 * failed.
 *
 * Renders nothing at all when translation is off (FR-007).
 */
export function TranslationOverlay({
  archiveId,
  page,
  settings,
  enabled,
  loadPageImage,
}: {
  archiveId: string
  page: number
  settings: TranslationSettings | null
  /** Whether translation is on for this archive (per-archive/per-Tankoubon scope). */
  enabled: boolean
  loadPageImage?: () => Promise<ImageBitmap | HTMLImageElement>
}) {
  const { t } = useTranslation()
  const state = usePageTranslation({ archiveId, page, settings, enabled, loadPageImage })
  const backend = settings ? resolveActiveBackend(settings) : { category: "none" as const }

  if (state.status === "idle") return null

  if (state.status === "pending") {
    // Original page stays fully visible; only a corner chip is added (FR-012).
    return <TranslationLoadingIndicator />
  }

  if (state.status === "failed") {
    // Per-page indicator, and nothing else — the page underneath remains readable (FR-019/FR-020).
    const detail = t(`translation.error.${state.kind}`, {
      defaultValue: t("translation.status.unavailable"),
    })

    // FR-018: when the locally-hosted backend is unreachable, the action that actually helps is
    // the guided Private-Network-Access explanation, not a one-line failure chip.
    if (state.kind === "unreachable" && backend.category === "local") {
      return <LocalBackendGuidance detail={state.message} />
    }

    return (
      <div className="translation-failed" role="status">
        {t("translation.status.unavailable")}
        <span className="translation-failed__detail">{detail}</span>
      </div>
    )
  }

  return (
    <img
      className="translation-overlay__image"
      src={state.imageUrl}
      alt={`${t("reader.page")} ${page}`}
      draggable={false}
    />
  )
}

export default TranslationOverlay
