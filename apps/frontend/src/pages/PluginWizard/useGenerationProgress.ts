import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { GenerateProgressEvent } from "./generateStream"

/** One rendered line in the progress log — `"log"` for a `fetch_page`/`fetch_result` line,
 * `"text"` for a run of `content_delta` chunks. Kept as one ordered array (not two separate
 * accumulators) specifically so the model's own streamed commentary renders interleaved at the
 * point in time it actually arrived, not shoved to the bottom regardless of when it happened — a
 * real, observed bug: DeepSeek sometimes streams a short explanatory sentence (e.g. "I'll start by
 * fetching the API documentation...") *alongside* a round that also requests `fetch_page` tool
 * calls, and rendering that text in a separate always-last block made it look like the model had
 * said nothing until the very end, when it had actually explained its very first move. */
export type ProgressItem = { kind: "log"; text: string } | { kind: "text"; text: string }

/** Shared accumulator for `streamGenerate`'s progress events, used identically by `GenerationStep.
 * tsx`/`TrialRunResult.tsx`'s auto-fix/`RefinePanel.tsx`'s refine — one time-ordered log combining
 * which round happened and the model's own streamed commentary/answer, plus a live elapsed-time
 * counter (added once `GENERATE_TIMEOUT` was raised to 300s — a caller waiting up to 5 minutes
 * needs to see time actually passing, not just a static spinner). `start()` must be called at the
 * beginning of each new generation call (not just once per component mount) so a second generation
 * doesn't visually append onto the first's leftover log or keep its stale elapsed time; `stop()`
 * must be called once the call settles (success or error) to stop the ticking interval. */
export function useGenerationProgress() {
  const { t } = useTranslation()
  const [items, setItems] = useState<ProgressItem[]>([])
  const [elapsedSeconds, setElapsedSeconds] = useState(0)
  const startedAtRef = useRef<number | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const stop = useCallback(() => {
    if (intervalRef.current !== null) {
      clearInterval(intervalRef.current)
      intervalRef.current = null
    }
  }, [])

  const start = useCallback(() => {
    stop()
    setItems([])
    setElapsedSeconds(0)
    startedAtRef.current = Date.now()
    intervalRef.current = setInterval(() => {
      if (startedAtRef.current !== null) {
        setElapsedSeconds(Math.floor((Date.now() - startedAtRef.current) / 1000))
      }
    }, 1000)
  }, [stop])

  // Stops a stray interval if the component unmounts mid-generation (e.g. the user switches
  // types) rather than leaking it — `stop()` on the happy/error path already covers the normal
  // case, this is only the unmount safety net.
  useEffect(() => stop, [stop])

  const onProgress = useCallback(
    (event: GenerateProgressEvent) => {
      if (event.kind === "fetch_page") {
        setItems((prev) => [
          ...prev,
          { kind: "log", text: t("pluginWizard.streamFetchingPage", { url: event.url }) ?? event.url },
        ])
      } else if (event.kind === "fetch_result") {
        const key =
          event.status === "ok" ? "pluginWizard.streamFetchResultOk" : "pluginWizard.streamFetchResultFailed"
        setItems((prev) => [...prev, { kind: "log", text: t(key, { url: event.url }) ?? event.url }])
      } else if (event.kind === "content_delta") {
        // Appends onto the most recent item only if it's *already* a text run — a `content_delta`
        // arriving after a log line (a new round started) begins a new text run instead of
        // concatenating onto stale text from a different round.
        setItems((prev) => {
          const last = prev[prev.length - 1]
          if (last?.kind === "text") {
            return [...prev.slice(0, -1), { kind: "text", text: last.text + event.text }]
          }
          return [...prev, { kind: "text", text: event.text }]
        })
      }
    },
    [t],
  )

  return { items, elapsedSeconds, start, stop, onProgress }
}
