import { useEffect, useRef } from "react"

/**
 * Cross-tab library changes.
 *
 * A batch run mutates the library with plain `fetch`, so any other open tab keeps what it had
 * cached — including a selection drawer still listing archives that this tab just deleted. Two tabs share nothing but the server, and re-fetching everything on a timer to
 * discover that would cost more than the problem does, so the writer says so instead.
 */
export const DATA_CHANNEL = "lanrurugi-data"

export interface DataChange {
  /** Ids that no longer exist anywhere. */
  removed?: string[]
  /** Ids whose record changed (tags, categories, title, the "new" flag...) but still exist. */
  changed?: string[]
}

export function postDataChange(change: DataChange): void {
  const removed = change.removed?.length ?? 0
  const changed = change.changed?.length ?? 0
  if (removed === 0 && changed === 0) return
  let channel: BroadcastChannel | null = null
  try {
    channel = new BroadcastChannel(DATA_CHANNEL)
    channel.postMessage(change)
  } catch {
    // No BroadcastChannel here (an old browser, or the page opened from `file:`): the other tab
    // simply stays stale, which is exactly the state this replaces.
  } finally {
    channel?.close()
  }
}

/** Runs `handler` for every change another tab announces. */
export function useDataChange(handler: (change: DataChange) => void): void {
  const latest = useRef(handler)
  useEffect(() => {
    latest.current = handler
  })
  useEffect(() => {
    let channel: BroadcastChannel | null = null
    try {
      channel = new BroadcastChannel(DATA_CHANNEL)
    } catch {
      return
    }
    const listener = (event: MessageEvent<DataChange>) => latest.current(event.data)
    channel.addEventListener("message", listener)
    return () => {
      channel.removeEventListener("message", listener)
      channel.close()
    }
  }, [])
}
