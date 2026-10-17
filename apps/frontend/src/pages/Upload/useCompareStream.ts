import { useCallback, useRef, useState } from "react"

import type { CompareEvent, ComparisonResult, PageComparison } from "@/api/types"

/** Partial mirror of `ComparisonResult` for streaming render — `samples` is keyed by
 * `sample_index` (may have gaps), summary fields are `null` until the terminal `done` event lands. */
export interface StreamingCompareState {
  samples: (PageComparison | undefined)[]
  summary: Omit<ComparisonResult, "samples"> | null
}

const EMPTY_STATE: StreamingCompareState = { samples: [], summary: null }

/** Drives `GET /download_queue/{id}/compare/stream` via a plain `EventSource`. `start(id)`
 * (re-)opens the stream; a later `"precise"` event replaces a sample by `sample_index` in place. */
export function useCompareStream() {
  const [state, setState] = useState<StreamingCompareState>(EMPTY_STATE)
  const [error, setError] = useState<string | null>(null)
  const [finished, setFinished] = useState(false)
  const sourceRef = useRef<EventSource | null>(null)
  // A ref, not state: `done` and the trailing `onerror` can fire in the same tick, and a state
  // update isn't guaranteed committed yet when `onerror` reads it — caused false "connection lost" toasts.
  const finishedRef = useRef(false)

  const close = useCallback(() => {
    sourceRef.current?.close()
    sourceRef.current = null
  }, [])

  const start = useCallback(
    (id: string) => {
      close()
      setState(EMPTY_STATE)
      setError(null)
      setFinished(false)
      finishedRef.current = false

      const source = new EventSource(`/api/download_queue/${encodeURIComponent(id)}/compare/stream`)
      sourceRef.current = source

      source.addEventListener("sample", (ev) => {
        const event = JSON.parse((ev as MessageEvent<string>).data) as CompareEvent
        if (event.type !== "sample") return
        setState((prev) => {
          const samples = [...prev.samples]
          samples[event.sample_index] = event.sample
          return { ...prev, samples }
        })
      })

      source.addEventListener("done", (ev) => {
        const event = JSON.parse((ev as MessageEvent<string>).data) as CompareEvent
        if (event.type !== "done") return
        const { type: _type, ...summary } = event
        setState((prev) => ({ ...prev, summary }))
        finishedRef.current = true
        setFinished(true)
        close()
      })

      source.addEventListener("error", (ev) => {
        const messageEvent = ev as MessageEvent<string> | undefined
        if (messageEvent?.data) {
          setError(messageEvent.data)
          close()
        }
      })

      source.onerror = () => {
        // Fires on both ordinary stream-end and a real dropped connection; finishedRef distinguishes them.
        if (!finishedRef.current) {
          setError((prevError) => prevError ?? "Connection to the comparison stream was lost.")
        }
        close()
      }
    },
    [close],
  )

  return { state, error, finished, start, close }
}
