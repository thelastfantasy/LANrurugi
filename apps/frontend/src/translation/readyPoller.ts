// Turns the server's "not ready yet" response into a real ready→swap transition (T042, FR-012).
//
// The requirement is specific: reaching a page before its look-ahead finished must show the
// original page *immediately* with a non-obscuring indicator, then swap in the translated version
// seamlessly once ready. That means neither a blocking wait nor a silent "it just changes later" —
// so something has to actively watch for readiness, which is this.

import { fetchPageTranslation, type PageTranslationResult } from "./api"

/** Backoff bounds. Starts responsive (a page often finishes moments after being reached) and eases
 * off so a slow backend isn't hammered. */
const INITIAL_INTERVAL_MS = 400
const MAX_INTERVAL_MS = 3000
const BACKOFF_FACTOR = 1.4
/** Gives up after this long, leaving the original page readable rather than polling forever. */
const DEFAULT_TIMEOUT_MS = 120_000

export interface PollHandle {
  /** Stops polling. Idempotent — safe to call from an effect cleanup that may run twice. */
  cancel: () => void
}

export interface PollCallbacks {
  onReady: (blob: Blob) => void
  onUnavailable: (kind: string, message: string) => void
  /** Called when polling gives up without a result; the original page stays readable. */
  onTimeout?: () => void
}

/**
 * Polls until the page's translation is ready, then hands back the composited image.
 *
 * Returns a handle whose `cancel()` abandons the wait — used on page turn and unmount so a page
 * the user has moved past stops being watched (FR-015's spirit on the client side).
 */
export function pollUntilReady(
  archiveId: string,
  page: number,
  targetLanguage: string,
  callbacks: PollCallbacks,
  timeoutMs: number = DEFAULT_TIMEOUT_MS,
): PollHandle {
  let cancelled = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const deadline = Date.now() + timeoutMs

  const stop = () => {
    cancelled = true
    if (timer !== undefined) clearTimeout(timer)
  }

  const tick = async (interval: number) => {
    if (cancelled) return

    if (Date.now() > deadline) {
      stop()
      callbacks.onTimeout?.()
      return
    }

    let result: PageTranslationResult
    try {
      result = await fetchPageTranslation(archiveId, page, targetLanguage)
    } catch (e) {
      // A network blip mid-poll isn't a translation failure; keep waiting rather than declaring
      // the page permanently unavailable.
      result = { status: "pending" }
      if (e instanceof Error && e.name === "AbortError") {
        stop()
        return
      }
    }

    if (cancelled) return

    if (result.status === "ready") {
      stop()
      callbacks.onReady(result.blob)
      return
    }

    if (result.status === "unavailable") {
      stop()
      callbacks.onUnavailable(result.kind, result.message)
      return
    }

    const next = Math.min(interval * BACKOFF_FACTOR, MAX_INTERVAL_MS)
    timer = setTimeout(() => void tick(next), interval)
  }

  void tick(INITIAL_INTERVAL_MS)

  return { cancel: stop }
}
