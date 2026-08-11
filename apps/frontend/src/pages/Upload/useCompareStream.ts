import { useCallback, useRef, useState } from "react"

import type { CompareEvent, ComparisonResult, PageComparison } from "@/api/types"

/** Everything `ComparisonResultModal` needs to render while a comparison is still streaming in —
 * a partial mirror of `ComparisonResult` where `samples` is keyed by `sample_index` (may contain
 * gaps while phase-1 coarse events are still arriving out of network order) and the summary fields
 * (`recommendation`, `a_entries`, etc.) are `null` until the terminal `done` event lands. Not the
 * same shape as `ComparisonResult` itself — the modal only ever renders once `summary` is non-null,
 * same as it always required a full `ComparisonResult` before this streaming rework. */
export interface StreamingCompareState {
  samples: (PageComparison | undefined)[]
  summary: Omit<ComparisonResult, "samples"> | null
}

const EMPTY_STATE: StreamingCompareState = { samples: [], summary: null }

/** Drives `GET /download_queue/{id}/compare/stream` (issue #77's two-phase SSE design — see
 * `lanrurugi_imgcompare::compare_archives_streaming`'s own docs for the full phase breakdown) via
 * a plain `EventSource`, not TanStack Query — `EventSource` is callback-driven, not an
 * `AsyncIterable`, and TanStack Query's own `streamedQuery` helper (the one construct it has for
 * incremental data) is a `useQuery` primitive that needs an `AsyncIterable` source and doesn't fit
 * this hook's "open on first byte, keep mutating in place" usage anyway.
 *
 * `start(id)` (re-)opens the stream for a given queue item; `close()` tears it down early (e.g. the
 * modal was dismissed before the stream finished) — the caller (`QueueItemRow`) owns when a
 * comparison starts, this hook only owns the connection and the incrementally-assembled state.
 * `state.summary` staying `null` is exactly "not ready to render the modal yet" — the caller opens
 * the modal the moment `state.samples` gets its first defined entry (see `QueueItemRow`'s own
 * `handleCompare`), not when `summary` appears; a `"precise"` event later replaces a sample already
 * shown in place (matched by `sample_index`), never appended as a new entry. */
export function useCompareStream() {
  const [state, setState] = useState<StreamingCompareState>(EMPTY_STATE)
  const [error, setError] = useState<string | null>(null)
  const [finished, setFinished] = useState(false)
  const sourceRef = useRef<EventSource | null>(null)
  // A plain ref, not React state — `done`'s handler and the browser's own trailing `onerror` (see
  // that handler's own docs below) can both fire within the same synchronous tick, and a `finished`
  // *state* update isn't guaranteed to have committed yet by the time `onerror` reads it, which
  // caused a real false-positive "Connection to the comparison stream was lost" toast on every
  // otherwise-successful comparison (confirmed live: server logs showed the stream completing
  // cleanly — 6 coarse + 1 precise sample, no error — every time this fired). A ref is read/written
  // synchronously, so `onerror` always sees whatever `done` just set, no matter the render timing.
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

      // A real, distinguishable server-side failure is its own named `event: error` SSE message
      // (`compare_queue_item_stream`'s own `Err(e)` branch) carrying `ImgCompareError`'s message
      // text as `data` — handled here. This is NOT the same thing as `source.onerror` below, which
      // is the browser's generic *connection-level* error/close callback and never carries `data`
      // at all (confirmed: `EventSource`'s spec-level `error` event is a plain `Event`, not a
      // `MessageEvent`) — despite both being dispatched under the type name `"error"`, they're
      // reached via different registration APIs and fire for different reasons.
      source.addEventListener("error", (ev) => {
        const messageEvent = ev as MessageEvent<string> | undefined
        if (messageEvent?.data) {
          setError(messageEvent.data)
          close()
        }
      })

      source.onerror = () => {
        // Fires on ordinary stream-end too (the server closes the HTTP response right after
        // `done`, which `EventSource` can't distinguish from a genuine drop at the protocol level)
        // AND on a real dropped connection before `done` ever arrived — only the latter is worth
        // surfacing, distinguished via `finishedRef` (see its own docs above for why a ref, not
        // state, is required here).
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
