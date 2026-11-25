import { useCallback, useEffect, useRef, useState } from "react"

import { applySample, normaliseSample, normaliseSnapshot } from "./parse"
import type { ResourceSample, ResourceSnapshot } from "./types"

/** Upper bound on the in-memory series: 1800 samples at 1 Hz is thirty minutes, and the client is
 * the only place a curve is kept — the server never writes history to disk (FR-011). */
export const RESOURCE_HISTORY_LIMIT = 1800

export type ResourceStreamStatus = "idle" | "connecting" | "live" | "error"

export interface ResourceStream {
  snapshot: ResourceSnapshot | null
  samples: ResourceSample[]
  status: ResourceStreamStatus
  error: string | null
}

/** Live samples over SSE (`GET /api/resources/stream`).
 *
 * Mounting is the subscription: there is no `enabled` flag to get wrong, because the panel's own body
 * is unmounted when the section closes (FR-001). Unmount closes the `EventSource`, the server drops
 * its last observer and stops sampling, so "closed means zero cost" needs no timer. A hidden tab
 * closes the connection too, rather than streaming at a page nobody is looking at. */
export function useResourceStream(intervalMs: number): ResourceStream {
  const [snapshot, setSnapshot] = useState<ResourceSnapshot | null>(null)
  const [samples, setSamples] = useState<ResourceSample[]>([])
  // "connecting" is the initial state rather than something a connect() call sets: a setState in an
  // effect body is a cascading render, and the first frame always arrives from an event anyway.
  const [status, setStatus] = useState<ResourceStreamStatus>("connecting")
  const [error, setError] = useState<string | null>(null)
  const sourceRef = useRef<EventSource | null>(null)

  const close = useCallback(() => {
    sourceRef.current?.close()
    sourceRef.current = null
  }, [])

  const connect = useCallback(() => {
    close()
    const source = new EventSource(`/api/resources/stream?interval_ms=${intervalMs}`)
    sourceRef.current = source

    source.addEventListener("open", () => {
      setStatus("live")
      setError(null)
    })

    source.addEventListener("snapshot", (event) => {
      try {
        setSnapshot(normaliseSnapshot(JSON.parse((event as MessageEvent<string>).data) as ResourceSnapshot))
        setStatus("live")
      } catch {
        setError("the snapshot frame was not valid JSON.")
      }
    })

    source.addEventListener("sample", (event) => {
      try {
        const sample = normaliseSample(JSON.parse((event as MessageEvent<string>).data) as ResourceSample)
        setSnapshot((current) => (current ? applySample(current, sample) : current))
        setSamples((current) => {
          const next = current.length >= RESOURCE_HISTORY_LIMIT ? current.slice(1) : current.slice()
          next.push(sample)
          return next
        })
      } catch {
        setError("a sample frame was not valid JSON.")
      }
    })

    source.addEventListener("error", () => {
      // EventSource retries on its own; surface the state without tearing the connection down, so a
      // server restart reconnects instead of leaving a dead panel.
      setStatus("error")
    })
  }, [close, intervalMs])

  useEffect(() => {
    if (typeof document !== "undefined" && document.visibilityState === "hidden") {
      return
    }
    connect()
    return close
  }, [close, connect])

  useEffect(() => {
    function onVisibility() {
      if (document.visibilityState === "hidden") {
        close()
        setStatus("idle")
      } else {
        connect()
      }
    }
    document.addEventListener("visibilitychange", onVisibility)
    return () => document.removeEventListener("visibilitychange", onVisibility)
  }, [close, connect])

  return { snapshot, samples, status, error }
}
