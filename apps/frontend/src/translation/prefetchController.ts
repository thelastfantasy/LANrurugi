// Abandons in-flight look-ahead work when the reader navigates away (T046, FR-015).
//
// Without this, closing the reader mid-window would leave requests running that can still cost
// money on a metered backend, for pages nobody is going to look at. Every request this controller
// issues carries an `AbortSignal` it owns, so abandoning is a single `abort()` rather than
// per-request bookkeeping.

/** One tracked in-flight request. */
interface Tracked {
  controller: AbortController
  page: number
}

/**
 * Owns the abort lifetime of a reading session's look-ahead requests.
 *
 * Scoped to one reader mount: create on mount, `abandonAll()` on unmount. Nothing survives, so a
 * new session can't inherit a previous one's stale requests.
 */
export class PrefetchController {
  private inFlight = new Map<number, Tracked>()

  /** A signal for a page's request. Replaces (and aborts) any existing request for that page, so
   * repeated scheduling of the same page can't pile up. */
  signalFor(page: number): AbortSignal {
    this.abandonPage(page)
    const controller = new AbortController()
    this.inFlight.set(page, { controller, page })
    return controller.signal
  }

  /** Marks a page's request finished — it no longer needs aborting. */
  settle(page: number): void {
    this.inFlight.delete(page)
  }

  /** Aborts one page's request, e.g. because it fell out of the look-ahead window. */
  abandonPage(page: number): void {
    const tracked = this.inFlight.get(page)
    if (!tracked) return
    tracked.controller.abort()
    this.inFlight.delete(page)
  }

  /** Aborts every page outside the given window — called on page turn so the window slides
   * forward without leaving stragglers behind it. */
  retainOnly(pages: number[]): void {
    const keep = new Set(pages)
    for (const page of [...this.inFlight.keys()]) {
      if (!keep.has(page)) this.abandonPage(page)
    }
  }

  /** Abandons everything (FR-015). Called on reader unmount/navigate-away. */
  abandonAll(): void {
    for (const tracked of this.inFlight.values()) {
      tracked.controller.abort()
    }
    this.inFlight.clear()
  }

  get inFlightCount(): number {
    return this.inFlight.size
  }
}
