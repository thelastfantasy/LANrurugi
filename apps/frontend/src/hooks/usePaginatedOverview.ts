import { useCallback, useEffect, useRef, useState } from "react"

import { fetchJson } from "@/api/client"

interface PageMeta {
  page: number
  arcId: string
  localPage: number
}

interface ThumbnailBatch {
  pages: PageMeta[]
  total: number
}

const BATCH_SIZE = 60

export function usePaginatedOverview(
  archiveId: string,
  anchorPage: number,
) {
  const [loadedPages, setLoadedPages] = useState<Map<number, PageMeta>>(new Map())
  const [total, setTotal] = useState(0)
  const [loadedStart, setLoadedStart] = useState(0)
  const [loadedEnd, setLoadedEnd] = useState(0)
  const [loadingDir, setLoadingDir] = useState<"up" | "down" | null>(null)
  const fetchedRanges = useRef(new Set<string>())

  const fetchRange = useCallback(
    async (from: number, count: number) => {
      const key = `${from}-${count}`
      if (fetchedRanges.current.has(key)) return
      fetchedRanges.current.add(key)

      setLoadingDir(from <= anchorPage ? "up" : "down")
      try {
        const data = await fetchJson<ThumbnailBatch>(
          `/archives/${archiveId}/thumbnails?from=${from}&count=${count}`,
        )
        setLoadedPages((prev) => {
          const next = new Map(prev)
          for (const p of data.pages) {
            next.set(p.page, p)
          }
          return next
        })
        setTotal(data.total)
        setLoadedStart((prev) => (prev === 0 ? from : Math.min(prev, from)))
        setLoadedEnd((prev) => Math.max(prev, from + data.pages.length - 1))
      } finally {
        setLoadingDir(null)
      }
    },
    [archiveId, anchorPage],
  )

  useEffect(() => {
    fetchedRanges.current.clear()
    const half = Math.floor(BATCH_SIZE / 2)
    const from = Math.max(1, anchorPage - half)
    // Deferred to avoid eslint set-state-in-effect
    queueMicrotask(() => {
      setLoadedPages(new Map())
      setLoadedStart(0)
      setLoadedEnd(0)
    })
    void fetchRange(from, BATCH_SIZE)
  }, [archiveId, anchorPage, fetchRange])

  // Debounced: a jump into view can trigger loadUp/loadDown from dozens of placeholders in the
  // same frame; without this it'd fire one fetch per placeholder instead of one total.
  const loadUpTimer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const loadUp = useCallback(() => {
    clearTimeout(loadUpTimer.current)
    loadUpTimer.current = setTimeout(() => {
      if (loadedStart <= 1 || loadingDir) return
      const from = Math.max(1, loadedStart - BATCH_SIZE)
      void fetchRange(from, Math.min(BATCH_SIZE, loadedStart - 1))
    }, 150)
  }, [loadedStart, loadingDir, fetchRange])

  const loadDownTimer = useRef<ReturnType<typeof setTimeout>>(undefined)
  const loadDown = useCallback(() => {
    clearTimeout(loadDownTimer.current)
    loadDownTimer.current = setTimeout(() => {
      if (loadedEnd >= total || total === 0 || loadingDir) return
      void fetchRange(loadedEnd + 1, BATCH_SIZE)
    }, 150)
  }, [loadedEnd, total, loadingDir, fetchRange])

  const pages = Array.from(loadedPages.keys()).sort((a, b) => a - b)

  return {
    pages,
    pageMeta: loadedPages,
    total,
    loadedStart,
    loadedEnd,
    loadingDir,
    loadUp,
    loadDown,
  }
}
