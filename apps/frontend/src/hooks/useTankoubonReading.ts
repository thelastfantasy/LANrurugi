import { useQueries } from "@tanstack/react-query"

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

/** Reads a Tankoubon as one concatenated book by splicing member archives' pages in order. */
export function useTankoubonReading(tankId: string | null) {
  const full = useTankoubonFull(tankId)
  const members = full.data?.result.full_data ?? []
  const memberIds = members.map((m) => m.arcid)

  const pageQueries = useQueries({
    queries: memberIds.map((id) => ({
      queryKey: ["archive-pages", id],
      queryFn: () => fetchJson<ArchiveFilesResponse>(`/archives/${id}/files`),
      enabled: tankId !== null,
    })),
  })

  const isLoading = full.isLoading || pageQueries.some((q) => q.isLoading)
  const isError = full.isError || pageQueries.some((q) => q.isError)
  const error = full.error ?? pageQueries.find((q) => q.error)?.error

  const chapters: TankoubonChapter[] = []
  const pages: ArchivePage[] = []
  let toc: TocEntry[] = []

  if (!isLoading && !isError) {
    const chapterNames = full.data?.result.chapter_names ?? []
    const nameById = new Map(chapterNames.map((c) => [c.id, c.name]))
    let offset = 0
    members.forEach((member, i) => {
      const memberPages = pageQueries[i]?.data?.pages ?? []
      if (memberPages.length === 0) return
      const startPage = offset + 1
      const endPage = offset + memberPages.length
      const displayTitle = nameById.get(member.arcid) || member.title
      chapters.push({ arcId: member.arcid, title: displayTitle, startPage, endPage })
      pages.push(...memberPages)
      toc.push({ name: displayTitle, page: startPage, synthetic: true })
      for (const entry of member.toc) {
        toc.push({ name: entry.name, page: startPage + entry.page - 1 })
      }
      offset = endPage
    })
  }

  /** Maps a global page to its member archive + local page, or `null` if out of range. */
  function getArchiveForPage(globalPage: number): { arcId: string; localPage: number } | null {
    const chapter = chapters.find((c) => globalPage >= c.startPage && globalPage <= c.endPage)
    if (!chapter) return null
    return { arcId: chapter.arcId, localPage: globalPage - chapter.startPage + 1 }
  }

  const tank = full.data?.result
  const metadata: { isLoading: boolean; isError: boolean; error: unknown; data: ArchiveMetadata | undefined } = {
    isLoading,
    isError,
    error,
    data:
      tank && !isLoading && !isError
        ? {
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
        : undefined,
  }

  const pagesResult: { isLoading: boolean; isError: boolean; error: unknown; data: ArchiveFilesResponse | undefined } = {
    isLoading,
    isError,
    error,
    data: !isLoading && !isError ? { job: 0, pages } : undefined,
  }

  return { metadata, pages: pagesResult, chapters, getArchiveForPage }
}
