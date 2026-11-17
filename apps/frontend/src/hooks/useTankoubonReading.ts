import { useQueries } from "@tanstack/react-query"
import { useMemo } from "react"

import { fetchJson } from "@/api/client"
import { useTankoubonFull } from "@/api/hooks"
import type { ArchiveFilesResponse, ArchiveMetadata, ArchivePage, TocEntry } from "@/api/types"

export interface TankoubonChapter {
  arcId: string
  title: string
  /** Global page numbers, not this archive's local ones. */
  startPage: number
  endPage: number
}

/** A member whose pages could not be listed (corrupt, deleted, or no longer readable). */
export interface TankoubonUnreadableChapter {
  arcId: string
  title: string
  error: unknown
}

/** Reads a Tankoubon as one concatenated book by splicing member archives' pages in order. */
export function useTankoubonReading(tankId: string | null) {
  const full = useTankoubonFull(tankId)

  // Keep the member list/query options stable so `useQueries` (and therefore the reader's
  // `pages.data` identity) does not get a brand-new object on every React render. A stable
  // `pages.data` is important for the reader's debounced prefetch effect: otherwise every
  // re-render (dimensions arriving, file sizes arriving, etc.) resets the 300ms prefetch timer.
  const members = useMemo(
    () => full.data?.result.full_data ?? [],
    [full.data],
  )
  const memberIds = useMemo(() => members.map((m) => m.arcid), [members])
  const queryOptions = useMemo(
    () => memberIds.map((id) => ({
      queryKey: ["archive-pages", id],
      queryFn: () => fetchJson<ArchiveFilesResponse>(`/archives/${id}/files`),
      enabled: tankId !== null,
    })),
    [memberIds, tankId],
  )

  // Use `combine` so `useQueries` returns one stable combined object. Without combine it returns a
  // brand-new array on every render even when the underlying query results are unchanged; that
  // instability would make the derived `pages`/`pages.data` change on every reader re-render and
  // reset the debounced prefetch timer just from dimensions/file-sizes arriving.
  const pageQueries = useQueries({
    queries: queryOptions,
    combine: (results) => ({
      data: results.map((q) => q.data),
      errors: results.map((q) => q.error),
      someLoading: results.some((q) => q.isLoading),
    }),
  })

  // A member that cannot be listed (a corrupt archive, one deleted while it was in the tankoubon)
  // must not take the whole book down with it: the rest of the book is still readable, so the
  // failing chapters are skipped and reported instead. Only when *every* member fails is this a
  // failure of the tankoubon itself.
  const failedMembers = members.filter((_, i) => pageQueries.errors[i])
  const isLoading = full.isLoading || pageQueries.someLoading
  const isError =
    full.isError || (members.length > 0 && failedMembers.length === members.length)
  const error = full.error ?? pageQueries.errors.find((e) => e)

  const { chapters, pages, toc, unreadableChapters } = useMemo(() => {
    const chapters: TankoubonChapter[] = []
    const pages: ArchivePage[] = []
    const unreadableChapters: TankoubonUnreadableChapter[] = []
    let toc: TocEntry[] = []

    if (!isLoading && !isError) {
      const chapterNames = full.data?.result.chapter_names ?? []
      const nameById = new Map(chapterNames.map((c) => [c.id, c.name]))
      let offset = 0
      members.forEach((member, i) => {
        const displayTitle = nameById.get(member.arcid) || member.title
        const memberPages = pageQueries.data[i]?.pages ?? []
        if (memberPages.length === 0) {
          if (pageQueries.errors[i]) {
            unreadableChapters.push({
              arcId: member.arcid,
              title: displayTitle,
              error: pageQueries.errors[i],
            })
          }
          return
        }
        const startPage = offset + 1
        const endPage = offset + memberPages.length
        chapters.push({ arcId: member.arcid, title: displayTitle, startPage, endPage })
        pages.push(...memberPages)
        toc.push({ name: displayTitle, page: startPage, synthetic: true })
        for (const entry of member.toc) {
          toc.push({ name: entry.name, page: startPage + entry.page - 1 })
        }
        offset = endPage
      })
    }

    return { chapters, pages, toc, unreadableChapters }
  }, [full.data, pageQueries, members, isLoading, isError])

  /** Maps a global page to its member archive + local page, or `null` if out of range. */
  function getArchiveForPage(globalPage: number): { arcId: string; localPage: number } | null {
    const chapter = chapters.find((c) => globalPage >= c.startPage && globalPage <= c.endPage)
    if (!chapter) return null
    return { arcId: chapter.arcId, localPage: globalPage - chapter.startPage + 1 }
  }

  const tank = full.data?.result
  const metadataData = useMemo<ArchiveMetadata | undefined>(() => {
    if (!tank || isLoading || isError) return undefined
    return {
      arcid: tank.id,
      title: tank.name,
      filename: "",
      tags: tank.tags,
      summary: tank.summary,
      isnew: false,
      extension: ".tank",
      progress: tank.progress,
      pagecount: pages.length,
      lastreadtime: 0,
      size: members.reduce((sum, m) => sum + m.size, 0),
      toc,
      archive_count: members.length,
    }
  }, [tank, isLoading, isError, pages.length, toc, members])

  const pagesData = useMemo<ArchiveFilesResponse | undefined>(
    () => (!isLoading && !isError ? { job: 0, pages } : undefined),
    [isLoading, isError, pages],
  )

  const metadata: { isLoading: boolean; isError: boolean; error: unknown; data: ArchiveMetadata | undefined } = {
    isLoading,
    isError,
    error,
    data: metadataData,
  }

  const pagesResult: { isLoading: boolean; isError: boolean; error: unknown; data: ArchiveFilesResponse | undefined } = {
    isLoading,
    isError,
    error,
    data: pagesData,
  }

  return { metadata, pages: pagesResult, chapters, unreadableChapters, getArchiveForPage }
}
