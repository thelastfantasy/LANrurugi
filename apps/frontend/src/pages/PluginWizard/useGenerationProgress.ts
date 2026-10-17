import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { GenerateProgressEvent } from "./generateStream"

/** One rendered line in the progress log: `"log"` for a `fetch_page`/`fetch_result` line, `"text"`
 * for a run of `content_delta` chunks — ordered so streamed commentary renders interleaved. */
export type ProgressItem = { kind: "log"; text: string } | { kind: "text"; text: string }

/** Shared accumulator for `streamGenerate`'s progress events. `start()` must be called at the
 * beginning of each new call; `stop()` once it settles, to stop the ticking interval. */
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

  // Unmount safety net in case the component unmounts mid-generation.
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
        // Only appends onto the last item if it's already a text run, so a delta after a log
        // line (new round) starts a fresh text run instead of concatenating onto stale text.
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
