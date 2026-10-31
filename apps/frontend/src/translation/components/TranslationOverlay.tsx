import "./TranslationOverlay.css"

import { useEffect, useRef } from "react"
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
  refreshToken,
}: {
  archiveId: string
  page: number
  settings: TranslationSettings | null
  /** Whether translation is on for this archive (per-archive/per-Tankoubon scope). */
  enabled: boolean
  loadPageImage?: () => Promise<ImageBitmap | HTMLImageElement>
  /** See `usePageTranslation`'s own docs: bumped to re-run this page without a reload. */
  refreshToken?: number
}) {
  const { t } = useTranslation()
  const overlayRef = useOriginalPageBox(page)
  const state = usePageTranslation({
    archiveId,
    page,
    settings,
    enabled,
    loadPageImage,
    refreshToken,
  })
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
      ref={overlayRef}
      className="translation-overlay__image"
      src={state.imageUrl}
      alt={`${t("reader.page")} ${page}`}
      draggable={false}
    />
  )
}

/**
 * Keeps the overlay exactly over the page it covers.
 *
 * CSS alone could not promise this: the reader's own `.reader-image` is `width:auto;height:auto`
 * with both max constraints, so its box inside the wrapper differs from the wrapper's own box by a
 * couple of pixels — and by a different amount in each display mode (fit-width, fit-height, original
 * size) and at each viewport size. Copying the original's measured box is the only rule that holds
 * in all of them; measured live 2026-10-31 on the same page in three modes.
 */
function useOriginalPageBox(page: number): React.RefObject<HTMLImageElement | null> {
  const overlayRef = useRef<HTMLImageElement | null>(null)

  useEffect(() => {
    const apply = () => {
      const img = overlayRef.current
      // The overlay is not necessarily a sibling of the page it covers (in manga mode it lives in
      // a different span), so look the page up by its own `data-page` — the attribute the reader
      // itself uses — rather than by DOM neighbourhood.
      const original =
        document.querySelector(`img.reader-image[data-page="${page}"]`) ??
        document.querySelector("img.reader-image")
      if (!img || !(original instanceof HTMLImageElement)) return
      const target = original.getBoundingClientRect()
      img.style.width = `${target.width}px`
      img.style.height = `${target.height}px`
      // Translate by the delta between where the overlay rendered and where the page actually is,
      // measured after clearing the previous transform. Doing it this way avoids reasoning about
      // which ancestor happens to be this element's containing block — the previous attempt got
      // that wrong and left a 1px offset at every size.
      img.style.transform = "none"
      const current = img.getBoundingClientRect()
      img.style.transform = `translate(${target.left - current.left}px, ${target.top - current.top}px)`
    }

    apply()
    const observer = new ResizeObserver(apply)
    const original =
      document.querySelector(`img.reader-image[data-page="${page}"]`) ??
      document.querySelector("img.reader-image")
    if (original) observer.observe(original)
    const parent = overlayRef.current?.parentElement
    if (parent) observer.observe(parent)
    window.addEventListener("resize", apply)
    return () => {
      observer.disconnect()
      window.removeEventListener("resize", apply)
    }
  }, [page])

  return overlayRef
}

export default TranslationOverlay
