import { useQueries } from "@tanstack/react-query"

import { fetchJson } from "@/api/client"
import { useTankoubonFull } from "@/api/hooks"
import type { ArchiveFilesResponse, ArchiveMetadata, TocEntry } from "@/api/types"

/** One member archive's own contribution to the concatenated page list — a slice of the whole
 * Tankoubon's global page range this archive's own pages occupy. */
export interface TankoubonChapter {
  arcId: string
  title: string
  /** 1-based, inclusive, in the Tankoubon's own global page numbering — not this archive's own
   * local page numbers. */
  startPage: number
  endPage: number
}

/** Reads a Tankoubon as one concatenated multi-archive book — matches real legacy's own
 * `reader_common.js` (`state.id.startsWith("TANK_")` branch): fetch every member archive's own
 * page list and splice them together in the Tankoubon's own stored (reading) order, with a
 * cumulative page-offset table (`chapters`) and a `getArchiveForPage` resolver standing in for
 * legacy's own `getArchiveForPage` — almost everything in `Reader.tsx` that needs to know "which
 * real archive (and which of *its own* local pages) does the page I'm looking at actually belong
 * to" (progress, bookmarks are the two exceptions — see their own call sites) goes through this.
 *
 * Shaped to match `useArchiveMetadata`/`useArchivePages`'s own return shapes as closely as
 * possible (`{ isLoading, isError, error, data }`) so `Reader.tsx` can feed either this hook's
 * output or the plain single-archive hooks' output into the same `metadata`/`pages` variables
 * without a fork in every downstream read.
 */
export function useTankoubonReading(tankId: string | null) {
  const full = useTankoubonFull(tankId)
  const members = full.data?.result.full_data ?? []
  const memberIds = members.map((m) => m.arcid)

  // One `/archives/{id}/files` fetch per member, in parallel — mirrors the `join_all` pattern
  // already used server-side for the same "don't serialize N archive lookups" reason (issue #66,
  // `common_member_tags`). Query key matches `useArchivePages`'s own (`['archive-pages', id]`),
  // so a member archive's page list is shared cache with a direct single-archive read of it.
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
  const pages: string[] = []
  let toc: TocEntry[] = []

  if (!isLoading && !isError) {
    let offset = 0
    members.forEach((member, i) => {
      const memberPages = pageQueries[i]?.data?.pages ?? []
      if (memberPages.length === 0) return
      const startPage = offset + 1
      const endPage = offset + memberPages.length
      chapters.push({ arcId: member.arcid, title: member.title, startPage, endPage })
      pages.push(...memberPages)
      // The member's own title stands in for legacy's own `buildTankChapters` top-level entry;
      // its own real ToC entries (if any) nest under that, offset-adjusted into the Tankoubon's
      // global page numbering — same two-level shape `ArchiveOverviewOverlay`'s chapter dropdown
      // already renders for a plain archive's own `toc`, just sourced from multiple archives.
      toc.push({ name: member.title, page: startPage, synthetic: true })
      for (const entry of member.toc) {
        toc.push({ name: entry.name, page: startPage + entry.page - 1 })
      }
      offset = endPage
    })
  }

  /** Maps a Tankoubon-global page number to the real member archive (and that archive's own
   * local page number) it falls in. `null` if `chapters` hasn't loaded yet or the page is out of
   * range (including every member having been dropped — see `TankoubonFullResponse`'s own docs
   * on `full_data` silently omitting missing member archives). */
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
