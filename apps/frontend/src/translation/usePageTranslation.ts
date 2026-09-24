// Reader-facing hook driving the translated-page overlay (T032/T042/T057).
//
// Its whole job is the three states FR-012/FR-019 require the reader to distinguish:
//   pending     → show the original page plus a non-obscuring indicator, then swap in seamlessly
//   ready       → show the translated page
//   failed      → show the original page plus a per-page "unavailable" indicator
//
// In every one of those states the *original* page stays readable. Nothing here ever blocks
// rendering, which is what keeps a translation problem from degrading Phase 1's core value
// (FR-020/SC-007).
//
// Lives in its own file rather than inside the Reader page component, per constitution
// Principle VII (a page's own file exports only that page's component).

import { useEffect, useRef, useState } from "react"

import { fetchPageTranslation, fetchTextRegions } from "./api"
import { LocalBackendUnreachableError, reportLocalTranslation, translateWithLocalBackend } from "./localBackend"
import { getOrCompositePage } from "./localCache"
import { type PollHandle,pollUntilReady } from "./readyPoller"
import { resolveActiveBackend, resolveTargetLanguage } from "./settings"
import type { TranslationPageState, TranslationSettings } from "./types"

export interface UsePageTranslationArgs {
  archiveId: string
  page: number
  /** Server-stored settings; `null` while still loading. */
  settings: TranslationSettings | null
  /** Whether translation is on for this archive (per-archive/per-Tankoubon scope, resolved by the
   * caller — see `useTranslationSettings`'s `TranslationScope`); `false` while still loading. */
  enabled: boolean
  /** Loads the original page image, used only on the client-compositing path. */
  loadPageImage?: () => Promise<ImageBitmap | HTMLImageElement>
}

/**
 * Resolves one page's translation state.
 *
 * Returns `{ status: 'idle' }` whenever translation is off — and, importantly, does no work at all
 * in that case: no fetch, no polling, no image decode (FR-007's "disabling restores prior
 * behaviour and performance").
 */
export function usePageTranslation({
  archiveId,
  page,
  settings,
  enabled,
  loadPageImage,
}: UsePageTranslationArgs): TranslationPageState {
  // Only *asynchronously produced* outcomes live in state. The idle/unconfigured/pending states
  // are pure functions of the props, so they're derived during render instead — setting them from
  // the effect body would render once with a stale value and then immediately again to correct it.
  const [resolved, setResolved] = useState<{ key: string; state: TranslationPageState } | null>(
    null,
  )
  const pollRef = useRef<PollHandle | null>(null)
  const objectUrlRef = useRef<string | null>(null)

  const backend = enabled && settings ? resolveActiveBackend(settings) : { category: "none" as const }
  const active = enabled && backend.category !== "none"

  useEffect(() => {
    // Revoke the previous page's blob URL before replacing it — a reader session turning through
    // hundreds of pages would otherwise leak one per page.
    const releaseObjectUrl = () => {
      if (objectUrlRef.current) {
        URL.revokeObjectURL(objectUrlRef.current)
        objectUrlRef.current = null
      }
    }

    pollRef.current?.cancel()
    pollRef.current = null
    releaseObjectUrl()

    if (!enabled || !settings) return

    const backend = resolveActiveBackend(settings)
    if (backend.category === "none") return

    const targetLanguage = resolveTargetLanguage(settings.targetLanguage)
    let cancelled = false

    const key = pageKey(archiveId, page)

    const setState = (next: TranslationPageState) => {
      if (!cancelled) setResolved({ key, state: next })
    }

    const publish = (blob: Blob) => {
      if (cancelled) return
      releaseObjectUrl()
      const url = URL.createObjectURL(blob)
      objectUrlRef.current = url
      setResolved({ key, state: { status: "ready", imageUrl: url } })
    }

    if (backend.category === "local") {
      // Local path: the browser translates and composites; the server never sees the call.
      void (async () => {
        try {
          const regions = await fetchTextRegions(archiveId, page)
          if (cancelled) return

          if (!regions) {
            // Detection hasn't reached this page yet — stay pending; the reader shows the
            // original page meanwhile.
            return
          }

          const translated = await translateWithLocalBackend(
            backend.config,
            regions.regions,
            regions.context,
            targetLanguage,
          )
          if (cancelled) return

          // Report back so terms this backend discovered reach the shared glossary (FR-007a).
          void reportLocalTranslation(
            archiveId,
            page,
            translated,
            targetLanguage,
            backend.config.identifier,
          )

          if (!loadPageImage) return
          const blob = await getOrCompositePage(
            {
              archiveId,
              pageNumber: page,
              targetLanguage,
              localProviderIdentifier: backend.config.identifier,
            },
            loadPageImage,
            translated,
          )
          publish(blob)
        } catch (e) {
          if (cancelled) return
          const blocked = e instanceof LocalBackendUnreachableError && e.likelyBlocked
          setState({
            status: "failed",
            kind: blocked ? "unreachable" : "malformed_response",
            message: e instanceof Error ? e.message : "local translation failed",
          })
        }
      })()

      return () => {
        cancelled = true
        releaseObjectUrl()
      }
    }

    // Cloud path: the server composites and caches; we either get it now or wait for look-ahead.
    void (async () => {
      try {
        const result = await fetchPageTranslation(archiveId, page, targetLanguage)
        if (cancelled) return

        if (result.status === "ready") {
          publish(result.blob)
          return
        }
        if (result.status === "unavailable") {
          setState({ status: "failed", kind: result.kind as never, message: result.message })
          return
        }

        // Still processing — watch for it rather than leaving the swap to chance (FR-012).
        pollRef.current = pollUntilReady(archiveId, page, targetLanguage, {
          onReady: publish,
          onUnavailable: (kind, message) => {
            if (!cancelled) setState({ status: "failed", kind: kind as never, message })
          },
          onTimeout: () => {
            if (!cancelled) {
              setState({
                status: "failed",
                kind: "unreachable",
                message: "translation timed out",
              })
            }
          },
        })
      } catch (e) {
        if (cancelled) return
        setState({
          status: "failed",
          kind: "unreachable",
          message: e instanceof Error ? e.message : "translation failed",
        })
      }
    })()

    return () => {
      cancelled = true
      pollRef.current?.cancel()
      pollRef.current = null
      releaseObjectUrl()
    }
  }, [archiveId, page, settings, enabled, loadPageImage])

  if (!enabled || !settings) return { status: "idle" }
  if (backend.category === "none") {
    // Enabled but unconfigured — guidance, not a silent failure (FR-021).
    return { status: "failed", kind: "not_configured", message: "not_configured" }
  }
  if (!active) return { status: "idle" }

  // A resolved outcome is tagged with the page it was produced for, so a page turn falls straight
  // back to `pending` on the very first render rather than briefly showing the previous page's
  // translated image over the new page.
  return resolved !== null && resolved.key === pageKey(archiveId, page)
    ? resolved.state
    : { status: "pending" }
}

/** Identifies which page a resolved outcome belongs to. */
function pageKey(archiveId: string, page: number): string {
  return `${archiveId}:${page}`
}
