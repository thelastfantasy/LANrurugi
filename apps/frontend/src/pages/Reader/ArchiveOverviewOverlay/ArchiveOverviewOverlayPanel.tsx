import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { FaFileArchive, FaPlus, FaShareAlt } from "react-icons/fa"
import { useNavigate } from "react-router-dom"

import {
  useAddTocEntry,
  useAddTocEntryForId,
  useAnalyzeArchiveSplit,
  useArchivePages,
  useArchiveSplitSuggestion,
  useArchiveSplitTree,
  useCreateCategory,
  useDeletePatch,
  useDeleteTankoubon,
  useExecuteArchiveSplit,
  useRemoveTocEntry,
  useRemoveTocEntryForId,
  useSetArchiveThumbnail,
  useSetTankoubonThumbnail,
  useSettings,
  useStampedPages,
  useStampedPagesForArchives,
  useUpdateTankoubon,
} from "@/api/hooks"
import type {
  ArchiveEntryInfo,
  ArchiveMetadata,
  ArchivePage,
  CategoryMetadata,
} from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { Button, IconButton, RatingWidget, Select } from "@/components/common-ui/Form"
import { confirmDialog, newCategoryDialog, promptDialog } from "@/dialog"
import { usePaginatedOverview } from "@/hooks/usePaginatedOverview"
import { useSupportsHover } from "@/hooks/useSupportsHover"
import type { TankoubonChapter } from "@/hooks/useTankoubonReading"
import { deleteArchiveToBin, sourceTagFrom } from "@/lib/deleteArchive"
import { routes } from "@/lib/routes"
import { isBookExtension } from "@/lib/utils/bookFormats"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"
import { sortCategories } from "@/lib/utils/sortCategories"
import { displayTocName, isReservedTocIdentifier } from "@/lib/utils/tocValidation"
import { DeleteConfirmDialog } from "@/pages/Library/DeleteConfirmDialog"
import { TankDeleteChoiceDialog } from "@/pages/Library/TankDeleteChoiceDialog"
import { EntryTreePopover } from "@/pages/Upload/EntryTreePopover"
import { toast } from "@/toast"

import { MAX_ROW_ASPECT, PageGridCell } from "./PageGridCell"
import { PageLightbox } from "./PageLightbox"
import { ChapterActionMenu } from "./shared"
import { TagsTable } from "./TagsTable"

/** Scroll-to-top floating button, visible only after scrolling down 300px. */
/** Touch screens: each row of the page grid is as tall as its shortest page, rather than legacy's
 * fixed 196px box that left a landscape page floating in empty space.
 *
 * A row is fitted once every thumbnail in it has loaded (so it changes height once, not per image),
 * from the images' own aspect ratio at the cell's fixed width; taller pages in the row are
 * letterboxed by the thumbnail's `object-fit: contain`. Rows are found by their top edge: cell
 * widths are fixed, so which cells share a row never depends on these heights.
 *
 * A row is capped at {@link MAX_ROW_ASPECT} times the cell width, so a row of long strips (a
 * webtoon) never fills the screen; those thumbnails show their top instead. */
function useFitRowsToShortestPage(grid: HTMLElement | null, enabled: boolean) {
  useEffect(() => {
    if (!grid || !enabled) return
    let frame = 0
    const fit = () => {
      const rows = new Map<number, HTMLElement[]>()
      for (const cell of grid.querySelectorAll<HTMLElement>(".overview-page-cell")) {
        const row = rows.get(cell.offsetTop) ?? []
        row.push(cell)
        rows.set(cell.offsetTop, row)
      }
      for (const cells of rows.values()) {
        const heights = cells.map((cell) => {
          const box = cell.querySelector<HTMLElement>(".id3")
          const img = box?.querySelector("img")
          return box && img?.complete && img.naturalWidth > 0
            ? box.clientWidth * Math.min(img.naturalHeight / img.naturalWidth, MAX_ROW_ASPECT)
            : null
        })
        const shortest = heights.every((h) => h !== null)
          ? Math.min(...(heights as number[]))
          : null
        for (const cell of cells) {
          const box = cell.querySelector<HTMLElement>(".id3")
          if (!box) continue
          // `!important`: legacy's own phone rule pins `div.id3` with one.
          if (shortest === null) box.style.removeProperty("height")
          else box.style.setProperty("height", `${Math.round(shortest)}px`, "important")
        }
      }
    }
    const schedule = () => {
      cancelAnimationFrame(frame)
      frame = requestAnimationFrame(fit)
    }
    schedule()
    // `load` does not bubble, but it does reach a capturing listener on an ancestor.
    grid.addEventListener("load", schedule, true)
    const resize = new ResizeObserver(schedule)
    resize.observe(grid)
    const children = new MutationObserver(schedule)
    children.observe(grid, { childList: true, subtree: true })
    return () => {
      cancelAnimationFrame(frame)
      grid.removeEventListener("load", schedule, true)
      resize.disconnect()
      children.disconnect()
    }
  }, [grid, enabled])
}

function ScrollToTopFab({ onJump }: { onJump: () => void }) {
  const { t } = useTranslation()
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const div = ref.current
    if (!div) return
    const overlay = div.closest("#archivePagesOverlay") as HTMLElement | null
    if (!overlay) return
    const scrollEl = overlay
    const el = div
    function check() { el.hidden = scrollEl.scrollTop < 300 }
    scrollEl.addEventListener("scroll", check, { passive: true })
    check()
    return () => scrollEl.removeEventListener("scroll", check)
  }, [])
  return (
    <div
      ref={ref}
      style={{ position: "sticky", bottom: 0, width: "100%", textAlign: "right", padding: "0 20px 12px 0", boxSizing: "border-box" }}
    >
      <button
        type="button"
        className="stdbtn"
        title={t("reader.scrollToTop") ?? undefined}
        onClick={() => {
          onJump()
          ;(ref.current?.closest("#archivePagesOverlay") as HTMLElement | undefined)?.scrollTo({ top: 0, behavior: "smooth" })
        }}
        style={{
          width: 32,
          height: 32,
          minWidth: 32,
          padding: 0,
          borderRadius: "50%",
          opacity: 0.85,
          background: "rgba(0,0,0,0.55)",
          color: "#fff",
          border: "none",
          boxShadow: "0 2px 6px rgba(0,0,0,0.3)",
        }}
      >
        <i className="fa fa-arrow-up" aria-hidden="true" />
      </button>
    </div>
  )
}

/** Placeholder for an unloaded page in the grid so the scrollbar reflects the true total. Its box
 * matches a loaded cell exactly (`overview-page-cell` fixes the width, `div.id3` the height), so
 * the grid never re-packs a row when the real thumbnail replaces it. */
function PagePlaceholder({ page: _page, onVisible }: { page: number; onVisible: () => void }) {
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const obs = new IntersectionObserver(
      ([entry]) => { if (entry.isIntersecting) onVisible() },
      { rootMargin: "240px" },
    )
    obs.observe(el)
    return () => obs.disconnect()
  }, [onVisible])
  return (
    <div
      ref={ref}
      className="id3 quick-thumbnail overview-page-cell"
      style={{ background: "rgba(128,128,128,0.06)", borderRadius: 4 }}
    />
  )
}

function DeletePatchButton({ archiveId, patchPageSet }: { archiveId: string; patchPageSet: Set<number> | null }) {
  const { t } = useTranslation()
  const delPatch = useDeletePatch(archiveId)

  function handleDelete() {
    const pages = patchPageSet ? [...patchPageSet].sort((a, b) => a - b) : []
    const groups: string[] = []
    let start = 0
    for (let i = 0; i < pages.length; i++) {
      if (i === 0 || pages[i] !== pages[i - 1] + 1) start = pages[i]
      const next = pages[i + 1]
      if (next === undefined || next !== pages[i] + 1) {
        groups.push(start === pages[i] ? `${start}` : `${start}–${pages[i]}`)
      }
    }
    if (!confirmDialog(
      <div>
        <div>{t("reader.deleteThePatchTheArchive")}</div>
        <div style={{ marginTop: 8 }}>{t("reader.patchedPages", { count: pages.length })}</div>
        {groups.length > 0 && (
          <ul style={{ margin: "4px auto 0", padding: 0, listStyle: "none", display: "inline-block", textAlign: "left" }}>
            {groups.map((g) => (
              <li key={g} style={{ padding: "2px 0" }}>• {t("common.pageN", { n: g })}</li>
            ))}
          </ul>
        )}
      </div>
    )) return
    void delPatch.mutateAsync()
  }

  return (
    <input
      className="stdbtn"
      type="button"
      style={{ width: "auto", minWidth: 110 }}
      value={(delPatch.isPending ? t("reader.deleting") : t("reader.deletePatch")) ?? undefined}
      disabled={delPatch.isPending}
      onClick={handleDelete}
    />
  )
}

/** Mirrors legacy's `#archivePagesOverlay` — thumbnail/admin options/categories/rating,
 * the tags table, then a thumbnail grid scoped to the current chapter (or the whole archive). */
function countSplitGroupFiles(
  entries: ArchiveEntryInfo[],
  sourceDirs: string[],
  sourceFiles: string[],
): number {
  const dirs = sourceDirs.map((d) => d.replace(/\/+$/, "")).filter(Boolean)
  return entries.filter(
    (e) =>
      e.is_regular_file &&
      (sourceFiles.includes(e.name) || dirs.some((d) => e.name.startsWith(`${d}/`))),
  ).length
}

export function ArchiveOverviewOverlay({
  archive,
  categories,
  loggedIn,
  currentPage,
  onClose,
  onSelectPage,
  autoFocus = false,
  resolvePage,
  tankChapters,
  tankPages,
  hasTankSplitCandidates,
  onOpenTankSplit,
  onShare,
  headless = false,
}: {
  archive: ArchiveMetadata
  categories: CategoryMetadata[] | undefined
  loggedIn: boolean
  currentPage: number
  onClose: () => void
  onSelectPage: (page: number) => void
  /** Scroll to and briefly highlight the current page's thumbnail after mount; only for a real
   * user click on the grid-toggle button, not when the overlay auto-opens. */
  autoFocus?: boolean
  /** Tankoubon mode only: resolves a global page number to its real owning archive + that
   * archive's local page number, since ToC/thumbnails are still per-archive resources. */
  resolvePage?: (globalPage: number) => { arcId: string; localPage: number } | null
  /** Tankoubon mode only: member archives' chapter list, for the "filter stamped pages" toggle. */
  tankChapters?: TankoubonChapter[]
  /** Tankoubon mode only: the concatenated multi-archive page list, passed to `PageLightbox`. */
  tankPages?: ArchivePage[]
  /** Tankoubon mode only: whether at least one member archive has a split suggestion. */
  hasTankSplitCandidates?: boolean
  /** Opens the Tankoubon batch split modal. */
  onOpenTankSplit?: () => void
  /** Opens the share dialog for this archive or tankoubon (017). */
  onShare?: () => void
  /** Headless reading hides the reader's own header, which is what normally carries the title;
   *  the overlay shows it itself then, exactly where narrow viewports already do. */
  headless?: boolean
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()
  const staticCategories = sortCategories((categories ?? []).filter((c) => !c.search))
  const archiveCategories = staticCategories.filter((c) => c.archives.includes(archive.arcid))
  const isTank = isTankoubonId(archive.arcid)
  const supportsHover = useSupportsHover()
  const [pageGridEl, setPageGridEl] = useState<HTMLDivElement | null>(null)
  useFitRowsToShortestPage(pageGridEl, !supportsHover)

  function resolve(page: number): { arcId: string; localPage: number } | null {
    return resolvePage ? resolvePage(page) : { arcId: archive.arcid, localPage: page }
  }

  // Legacy's `#filter-stamped` — marks stamped thumbnails and toggles a stamped-only view.
  // Tankoubon mode merges stamped pages across members, converted to global page numbers.
  const singleStampedPages = useStampedPages(isTank ? null : archive.arcid)
  const tankStampedPageQueries = useStampedPagesForArchives(isTank ? (tankChapters ?? []).map((c) => c.arcId) : [])
  const stampedPageSet = isTank
    ? new Set(
        (tankChapters ?? []).flatMap((chapter, i) =>
          (tankStampedPageQueries[i]?.data?.result ?? []).map((localPage) => String(chapter.startPage + Number(localPage) - 1)),
        ),
      )
    : new Set(singleStampedPages.data?.result ?? [])
  const [filterStamped, setFilterStamped] = useState(false)
  const [removeTocMenuAt, setRemoveTocMenuAt] = useState<DOMRect | null>(null)
  const [editTocMenuAt, setEditTocMenuAt] = useState<DOMRect | null>(null)
  const [lightboxPage, setLightboxPage] = useState<number | null>(null)

  const chapters = archive.toc.length > 0 ? archive.toc : null

  const currentChapter = chapters
    ? [...chapters].filter((c) => c.page <= currentPage).sort((a, b) => b.page - a.page)[0]
    : undefined

  /** Chapter name per start page, for the badge `PageGridCell` paints on that page's thumbnail.
   * Built over the whole ToC, not just the loaded slice: `pageMeta` decides what renders. Two
   * chapters sharing a start page keep the first one, matching the selector's own ordering. */
  const chapterNameByPage = new Map<number, string>()
  for (const entry of chapters ?? []) {
    if (!chapterNameByPage.has(entry.page)) chapterNameByPage.set(entry.page, displayTocName(entry.name, t))
  }

  const setThumbnail = useSetArchiveThumbnail(isTank ? "" : archive.arcid)
  const setTankoubonThumbnail = useSetTankoubonThumbnail(archive.arcid)
  const addTocEntry = useAddTocEntry(isTank ? "" : archive.arcid)
  const removeTocEntry = useRemoveTocEntry(isTank ? "" : archive.arcid)
  const addTocEntryForId = useAddTocEntryForId()
  const removeTocEntryForId = useRemoveTocEntryForId()
  const deleteTankoubon = useDeleteTankoubon()
  const [tankDeleteOpen, setTankDeleteOpen] = useState(false)
  const [archiveDeleteOpen, setArchiveDeleteOpen] = useState(false)
  const splitSuggestion = useArchiveSplitSuggestion(!isTank ? archive.arcid : null)
  const analyzeSplit = useAnalyzeArchiveSplit()
  const executeSplit = useExecuteArchiveSplit()
  const settings = useSettings()
  const [showSplitPanel, setShowSplitPanel] = useState(false)
  const splitTree = useArchiveSplitTree(!isTank && showSplitPanel ? archive.arcid : null)
  const [splitDeleteOriginal, setSplitDeleteOriginal] = useState(
    settings.data?.archive_split_delete_original_enabled ?? false,
  )
  const [splitStatus, setSplitStatus] = useState<string | null>(null)
  const [splitCreateTankoubon, setSplitCreateTankoubon] = useState(false)
  // Live SSE progress: the panel owns the numbers, the server only sends a stage code.
  const [splitProgress, setSplitProgress] = useState<number | null>(null)
  const [splitStage, setSplitStage] = useState<string | null>(null)
  const [splitDone, setSplitDone] = useState<number | null>(null)
  const [splitTotal, setSplitTotal] = useState<number | null>(null)
  const [splitCreated, setSplitCreated] = useState<number | null>(null)
  const updateTankoubon = useUpdateTankoubon(archive.arcid)

  const [thumbnailVersion, setThumbnailVersion] = useState(0)

  /** Legacy's `.set-thumbnail` handler. In Tankoubon mode always sets the Tankoubon's own cover;
   * the backend resolves the owning member archive. */
  function handleSetThumbnail(page: number) {
    const mutation = isTank ? setTankoubonThumbnail : setThumbnail
    mutation.mutate(page, {
      onSuccess: () => {
        setThumbnailVersion((v) => v + 1)
        toast({ text: t("reader.successfullySetPageNAs", { n: page }) ?? undefined })
      },
      onError: () => toast({ text: t("reader.errorUpdatingThumbnail") ?? undefined, icon: "error" }),
    })
  }

  /** Loops the prompt until the user cancels or enters something that isn't a reserved
   * internal-identifier-style string (`c1`-`c15`/`toc`). */
  async function promptTocTitle(defaultValue = ""): Promise<string | null> {
    let message = t("reader.enterATitleForThis") ?? ""
    let value = defaultValue
    for (;;) {
      const input = await promptDialog(message, value)
      if (input === null) return null
      const trimmed = input.trim()
      if (trimmed === "") return null
      if (isReservedTocIdentifier(trimmed)) {
        message = t("reader.isAReservedIdentifierAnd", { value: trimmed }) ?? ""
        value = trimmed
        continue
      }
      return trimmed
    }
  }

  /** Legacy's `.add-toc` handler. `page` is a global page number in Tankoubon mode; resolves to
   * the real owning archive first. */
  async function handleAddToc(page: number) {
    const title = await promptTocTitle()
    if (!title) return
    const target = resolve(page)
    if (!target) return
    const onError = () => toast({ text: t("reader.errorAddingRemovingChapter") ?? undefined, icon: "error" })
    if (isTank) {
      addTocEntryForId.mutate({ id: target.arcId, page: target.localPage, title }, { onError })
    } else {
      addTocEntry.mutate({ page: target.localPage, title }, { onError })
    }
  }

  /** Right-click quick-add shortcut for common preset chapter titles (封面/封底/目录/彩页/第N章). */
  function handleQuickAddToc(page: number, title: string) {
    const target = resolve(page)
    if (!target) return
    const onError = () => toast({ text: t("reader.errorAddingRemovingChapter") ?? undefined, icon: "error" })
    if (isTank) {
      addTocEntryForId.mutate({ id: target.arcId, page: target.localPage, title }, { onError })
    } else {
      addTocEntry.mutate({ page: target.localPage, title }, { onError })
    }
  }

  /** Legacy's `.edit-toc` handler — re-prompts with the existing name, re-adds at the same page
   * (upsert-by-page). Refuses to edit a reserved preset identifier (`c1`-`c20`/`toc`). */
  async function handleEditToc(entry = currentChapter) {
    if (!entry || isReservedTocIdentifier(entry.name)) return
    const title = await promptTocTitle(displayTocName(entry.name, t))
    if (!title) return
    const target = resolve(entry.page)
    if (!target) return
    const onError = () => toast({ text: t("reader.errorAddingRemovingChapter") ?? undefined, icon: "error" })
    if (isTank) {
      addTocEntryForId.mutate({ id: target.arcId, page: target.localPage, title }, { onError })
    } else {
      addTocEntry.mutate({ page: target.localPage, title }, { onError })
    }
  }

  /** Unlike legacy (which only ever targets the current chapter), the delete button opens
   * `ChapterActionMenu` listing every chapter, so any chapter can be deleted directly. */
  async function handleRemoveToc(entry: { page: number; name: string }) {
    if (!(await confirmDialog(t("reader.confirmDeleteNamed", { name: entry.name }) ?? "", true))) return
    const target = resolve(entry.page)
    if (!target) return
    const onError = () => toast({ text: t("reader.errorAddingRemovingChapter") ?? undefined, icon: "error" })
    if (isTank) {
      removeTocEntryForId.mutate({ id: target.arcId, page: target.localPage }, { onError })
    } else {
      removeTocEntry.mutate(target.localPage, { onError })
    }
  }

  async function addToCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: "PUT" })
    await queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  async function handleNewCategory() {
    const result = await newCategoryDialog()
    if (result === null) return
    try {
      const data = await createCategory.mutateAsync(result)
      if (!result.isDynamic) await addToCategory(data.category_id)
    } catch {
      toast({ heading: t("common.errorModifyingCategory") ?? undefined, icon: "error" })
    }
  }

  async function removeFromCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: "DELETE" })
    await queryClient.invalidateQueries({ queryKey: ["categories"] })
  }

  /** Deleting a Tankoubon only removes the grouping record, not member archives' files — a
   * milder operation than deleting a real archive, so it gets its own confirmation copy. */
  async function deleteArchive() {
    if (isTank) {
      setTankDeleteOpen(true)
      return
    }
    // The library's own confirmation: it asks whether subscriptions may fetch the work again.
    setArchiveDeleteOpen(true)
  }

  async function confirmArchiveDelete(blockResubscribe: boolean) {
    setArchiveDeleteOpen(false)
    const deleted = await deleteArchiveToBin({
      archiveId: archive.arcid,
      reconsiderSource: blockResubscribe ? undefined : sourceTagFrom(archive.tags),
      queryClient,
      t,
    })
    if (!deleted) return
    queryClient.invalidateQueries({ queryKey: ["archive", archive.arcid] })
    queryClient.invalidateQueries({ queryKey: ["archives"] })
    navigate(routes.library())
  }

  const [scrollAnchor, setScrollAnchor] = useState<number | null>(null)
  const {
    pages: paginatedPages,
    pageMeta,
    total: loadedTotal,
    loadedStart,
    loadedEnd,
    loadUp,
    loadDown,
  } = usePaginatedOverview(archive.arcid, scrollAnchor ?? (autoFocus ? (currentPage || 1) : 1))
  const pageCount = archive.pagecount

  const singlePagesForPatch = useArchivePages(isTank ? null : archive.arcid)
  const patchPageSet = (() => {
    const list = isTank ? tankPages : singlePagesForPatch.data?.pages
    if (!list) return null as Set<number> | null
    const s = new Set<number>()
    list.forEach((p, i) => { if (p.is_patch) s.add(i + 1) })
    return s
  })()

  const [highlightedPage, setHighlightedPage] = useState<number | null>(null)
  // The `scrollTop` this effect last set — or, before it has centred anything, the one the overlay
  // opened at. Kept in a ref rather than a local because the effect re-runs on every batch load.
  const appliedTopRef = useRef<number | null>(null)
  useEffect(() => {
    if (!autoFocus) return
    const section = document.getElementById("pages-section")
    const overlay = document.getElementById("archivePagesOverlay")
    if (!section || !overlay) return
    // A drag that starts before the current page's batch has arrived is the reader taking over just
    // as much as one after it: without a baseline, the first centring yanked the panel to the
    // current page in the middle of that drag (a slow drag would suddenly jump a long way).
    if (appliedTopRef.current === null) appliedTopRef.current = overlay.scrollTop

    // `scrollIntoView({ block: "center" })` centres against the overlay's box as it is at that one
    // instant. On a phone that box's height changes whenever the browser's own toolbar slides in or
    // out (the overlay is sized in `dvh`), so the page that was just centred drifts away from the
    // middle afterwards. Centre by hand instead and re-centre on every resize — the guard below
    // hands control back as soon as the reader scrolls, so this can never fight a manual drag.
    const centreOnPage = () => {
      const cell = document.querySelector<HTMLElement>(`[data-page-cell="${currentPage}"]`)
      if (!cell) return false
      // Anything but our own last write (or the opening position) means the reader took over: never
      // fight a manual scroll.
      if (appliedTopRef.current !== null && Math.abs(overlay.scrollTop - appliedTopRef.current) > 1) return true
      const box = overlay.getBoundingClientRect()
      const rect = cell.getBoundingClientRect()
      overlay.scrollTop += (rect.top - box.top) - (overlay.clientHeight - rect.height) / 2
      appliedTopRef.current = overlay.scrollTop
      return true
    }
    const tryCentre = () => {
      if (!centreOnPage()) return
      setHighlightedPage(currentPage)
    }
    const obs = new MutationObserver(tryCentre)
    const ro = new ResizeObserver(tryCentre)
    obs.observe(section, { childList: true, subtree: true })
    ro.observe(overlay)
    const immediate = setTimeout(tryCentre, 0)
    const clearTimer = setTimeout(() => setHighlightedPage(null), 3000)
    return () => {
      obs.disconnect()
      ro.disconnect()
      clearTimeout(immediate)
      clearTimeout(clearTimer)
    }
  }, [autoFocus, currentPage, paginatedPages])

  return (
    <>
      <div id="overlay-shade" style={{ display: "block", opacity: 0.6 }} onClick={onClose} />
      <div
        id="archivePagesOverlay"
        className="id1 base-overlay page-overlay"
        // Same lock as the reader's other overlays: the panel has its own scroller, and the reader
        // page behind it must not move while this is open.
        data-overlay="modal"
        style={{ padding: "0 16px", boxSizing: "border-box", overscrollBehavior: "contain" }}
      >
        <h2 className="ih" style={{ textAlign: "center" }}>
          {t("reader.archiveOverview")}
        </h2>
        {/* The reader's own header (which carries the title) is hidden at <=1000px and in headless
            mode — see `.overview-archive-title` in `index.css`. */}
        <div className={headless ? "overview-archive-title is-shown" : "overview-archive-title"}>
          {archive.title}
        </div>

        <div id="tagContainer" className="caption caption-tags caption-reader" style={{ maxWidth: "100%", boxSizing: "border-box", overflow: "hidden" }}>
          <br />
          <div style={{ marginBottom: 16 }}>
            <div className="id3 nocrop reader-thumbnail" style={{ maxWidth: 200 }}>
              <img
                alt=""
                src={`${isTank ? `/api/tankoubons/${archive.arcid}/thumbnail` : `/api/archives/${archive.arcid}/thumbnail`}${thumbnailVersion > 0 ? `?v=${thumbnailVersion}` : ""}`}
                style={{ maxWidth: "100%" }}
              />
            </div>

            {loggedIn && (
              <div style={{ display: "inline-block", verticalAlign: "middle" }}>
                <h2>{t("reader.adminOptions")}</h2>

                {/* `admin-actions` lets the pair share one row on a phone: `.stdbtn`'s theme-given
                    150px minimum cannot fit two of them in the panel's ~281px inner column. */}
                <div className="admin-actions" style={{ display: "flex", alignItems: "center", gap: 4, flexWrap: "wrap" }}>
                  <Button
                    variant="stdbtn"
                    style={archive.has_patch ? { width: "auto", minWidth: 110 } : undefined}
                    onClick={() => navigate(isTank ? routes.tankoubonEdit(archive.arcid) : routes.edit(archive.arcid))}
                  >
                    {isTank ? t("common.editTankoubon") : t("reader.editArchiveMetadata")}
                  </Button>
                  {/* Each theme's own destructive fill, same as the library's delete confirmations. */}
                  <Button
                    variant="stdbtn"
                    className="stdbtn-danger"
                    style={archive.has_patch ? { width: "auto", minWidth: 110 } : undefined}
                    onClick={() => void deleteArchive()}
                  >
                    {isTank ? t("common.deleteTankoubon") : t("common.deleteArchive")}
                  </Button>
                  {!isTank && archive.has_patch && <DeletePatchButton archiveId={archive.arcid} patchPageSet={patchPageSet} />}
                  {isTank && hasTankSplitCandidates && onOpenTankSplit && (
                    supportsHover ? (
                      <Tooltip
                        label={
                          <div>
                            <div style={{ fontWeight: 600, marginBottom: 4 }}>
                              {t("reader.archiveSplit")}
                            </div>
                            <div style={{ opacity: 0.8 }}>
                              {t("reader.archiveSplitTooltipDescription")}
                            </div>
                          </div>
                        }
                        anchor="element"
                        closeDelay={0}
                        wrapperStyle={{ display: "inline-flex", alignItems: "center", verticalAlign: "middle" }}
                      >
                        <Button
                          variant="stdbtn"
                          style={{ width: "auto", minWidth: 110 }}
                          onClick={onOpenTankSplit}
                        >
                          <FaFileArchive /> {t("reader.archiveSplit")}
                        </Button>
                      </Tooltip>
                    ) : (
                      <Button
                        variant="stdbtn"
                        style={{ width: "auto", minWidth: 110 }}
                        onClick={onOpenTankSplit}
                      >
                        <FaFileArchive /> {t("reader.archiveSplit")}
                      </Button>
                    )
                  )}
                  {/* An e-book has no inner files to regroup into separate zips. */}
                  {!isTank && !archive.has_split_suggestion && !isBookExtension(archive.extension) && (
                    supportsHover ? (
                      <Tooltip
                        label={
                          <div>
                            <div style={{ fontWeight: 600, marginBottom: 4 }}>
                              {t("reader.analyzeArchiveSplit")}
                            </div>
                            <div style={{ opacity: 0.8 }}>
                              {t("reader.analyzeArchiveSplitDescription")}
                            </div>
                          </div>
                        }
                        anchor="element"
                        closeDelay={0}
                        wrapperStyle={{ display: "inline-flex", alignItems: "center", verticalAlign: "middle" }}
                      >
                        <Button
                          variant="stdbtn"
                          style={{ width: "auto", minWidth: 140 }}
                          disabled={analyzeSplit.isPending}
                          onClick={() => {
                            analyzeSplit.mutate(archive.arcid, {
                              onSuccess: (data) => {
                                if (data.suggestion) {
                                  queryClient.invalidateQueries({ queryKey: ["archive", archive.arcid] })
                                  queryClient.invalidateQueries({ queryKey: ["archives"] })
                                  queryClient.invalidateQueries({ queryKey: ["archive-split-suggestion", archive.arcid] })
                                  setShowSplitPanel(true)
                                } else {
                                  // The split panel (and its status line) only exists once there is a suggestion, so
                                  // the "nothing to split" outcome has to be a toast or the click looks like it did nothing.
                                  toast({
                                    heading: t("reader.analyzeArchiveSplit") ?? undefined,
                                    text: t("reader.archiveSplitNotNeeded") ?? "",
                                    icon: "info",
                                  })
                                }
                              },
                              onError: (e) => {
                                toast({
                                  heading: t("reader.analyzeArchiveSplit") ?? undefined,
                                  text: String(e),
                                  icon: "error",
                                })
                              },
                            })
                          }}
                        >
                          {analyzeSplit.isPending ? (t("reader.running") ?? "…") : t("reader.analyzeArchiveSplit")}
                        </Button>
                      </Tooltip>
                    ) : (
                      <Button
                        variant="stdbtn"
                        style={{ width: "auto", minWidth: 140 }}
                        disabled={analyzeSplit.isPending}
                        onClick={() => {
                          analyzeSplit.mutate(archive.arcid, {
                            onSuccess: (data) => {
                              if (data.suggestion) {
                                queryClient.invalidateQueries({ queryKey: ["archive", archive.arcid] })
                                queryClient.invalidateQueries({ queryKey: ["archives"] })
                                queryClient.invalidateQueries({ queryKey: ["archive-split-suggestion", archive.arcid] })
                                setShowSplitPanel(true)
                              } else {
                                // The split panel (and its status line) only exists once there is a suggestion, so
                                // the "nothing to split" outcome has to be a toast or the click looks like it did nothing.
                                toast({
                                  heading: t("reader.analyzeArchiveSplit") ?? undefined,
                                  text: t("reader.archiveSplitNotNeeded") ?? "",
                                  icon: "info",
                                })
                              }
                            },
                            onError: (e) => {
                              toast({
                                heading: t("reader.analyzeArchiveSplit") ?? undefined,
                                text: String(e),
                                icon: "error",
                              })
                            },
                          })
                        }}
                      >
                        {analyzeSplit.isPending ? (t("reader.running") ?? "…") : t("reader.analyzeArchiveSplit")}
                      </Button>
                    )
                  )}
                  {!isTank && archive.has_split_suggestion && (
                    supportsHover ? (
                      <Tooltip
                        label={
                          <div>
                            <div style={{ fontWeight: 600, marginBottom: 4 }}>
                              {t("reader.archiveSplit")}
                            </div>
                            <div style={{ opacity: 0.8 }}>
                              {t("reader.archiveSplitTooltipDescription")}
                            </div>
                          </div>
                        }
                        anchor="element"
                        closeDelay={0}
                        wrapperStyle={{ display: "inline-flex", alignItems: "center", verticalAlign: "middle" }}
                      >
                        <Button
                          variant="stdbtn"
                          style={{ minWidth: 0, width: "2em", padding: 0, display: "inline-flex", alignItems: "center", justifyContent: "center" }}
                          onClick={() => setShowSplitPanel(true)}
                          aria-label={t("reader.archiveSplit") ?? undefined}
                        >
                          <FaFileArchive />
                        </Button>
                      </Tooltip>
                    ) : (
                      <Button
                        variant="stdbtn"
                        style={{ minWidth: 0, width: "2em", padding: 0, display: "inline-flex", alignItems: "center", justifyContent: "center" }}
                        onClick={() => setShowSplitPanel(true)}
                        aria-label={t("reader.archiveSplit") ?? undefined}
                      >
                        <FaFileArchive />
                      </Button>
                    )
                  )}
                </div>
                <br />

                {!isTank && showSplitPanel && (
                  <Modal onClose={() => setShowSplitPanel(false)} width={720} textAlign="left">
                    <h2 style={{ textAlign: "center" }}>{t("reader.archiveSplit")}</h2>
                    <div style={{ textAlign: "right", marginBottom: 4 }}>
                      <EntryTreePopover entries={splitTree.data?.entries ?? []} />
                    </div>
                    {splitSuggestion.data?.suggestion ? (
                      <>
                        <div style={{ margin: "8px 0", fontWeight: 600 }}>
                          {t("reader.archiveSplitSuggestionFound")}
                        </div>
                        <table className="itg" style={{ width: "100%", tableLayout: "auto" }}>
                          <thead>
                            <tr>
                              <th style={{ padding: "6px 8px", textAlign: "left" }}>{t("reader.splitOutputFile")}</th>
                              <th style={{ padding: "6px 8px", textAlign: "left", whiteSpace: "nowrap" }}>{t("reader.splitDescription")}</th>
                              <th style={{ padding: "6px 8px", textAlign: "right", whiteSpace: "nowrap" }}>{t("reader.splitItemCount")}</th>
                            </tr>
                          </thead>
                          <tbody>
                            {splitSuggestion.data.suggestion.split_groups.map((g) => (
                              <tr key={g.zip_name}>
                                <td style={{ padding: "6px 8px", textAlign: "left" }}>{g.zip_name}</td>
                                <td style={{ padding: "6px 8px", textAlign: "left", opacity: 0.8, whiteSpace: "nowrap" }}>{g.description}</td>
                                <td style={{ padding: "6px 8px", textAlign: "right" }}>
                                  {splitTree.data
                                    ? `${countSplitGroupFiles(splitTree.data.entries, g.source_dirs, g.source_files)} ${t("reader.splitItemsSuffix")}`
                                    : "…"}
                                </td>
                              </tr>
                            ))}
                          </tbody>
                        </table>
                        <label style={{ marginTop: 12, display: "block", textAlign: "center", cursor: "pointer" }}>
                          <input
                            type="checkbox"
                            checked={splitCreateTankoubon}
                            onChange={(e) => setSplitCreateTankoubon(e.target.checked)}
                          />{" "}
                          {t("reader.splitCreateTankoubonCheckbox")}
                        </label>
                        <label style={{ marginTop: 6, display: "block", textAlign: "center", cursor: "pointer" }}>
                          <input
                            type="checkbox"
                            checked={splitDeleteOriginal}
                            onChange={(e) => setSplitDeleteOriginal(e.target.checked)}
                          />{" "}
                          {t("reader.splitDeleteOriginalCheckbox")}
                        </label>
                        {splitDeleteOriginal && (
                          <div style={{ marginTop: 4, textAlign: "center", color: "red", fontWeight: 600 }}>
                            {t("reader.splitDeleteWarning")}
                          </div>
                        )}
                        <div style={{ marginTop: 16, textAlign: "center" }}>
                          <input
                            className="stdbtn"
                            type="button"
                            style={{ width: "auto", minWidth: 110 }}
                            value={(executeSplit.isPending ? t("reader.running") : t("reader.executeSplit")) ?? undefined}
                            disabled={executeSplit.isPending}
                            onClick={() => {
                              // Show the bar on the click, not on the first event: the POST
                              // answers with a job id only after the run has started, and by the
                              // time the stream connects the early stages are already gone — that
                              // left the bar appearing at whatever the last event said.
                              const groups = splitSuggestion.data?.suggestion?.split_groups.length ?? null
                              setSplitProgress(0)
                              setSplitStage("started")
                              setSplitDone(groups === null ? null : 0)
                              setSplitTotal(groups)
                              setSplitCreated(null)
                              void executeSplit
                                .mutateAsync({
                                  id: archive.arcid,
                                  deleteOriginal: splitDeleteOriginal,
                                  createTankoubon: splitCreateTankoubon,
                                })
                                .then((r) => {
                                  setSplitStatus(`${t("reader.splitJobStarted")}: ${r.job_id}`)
                                  const source = new EventSource(`/api/archives/${archive.arcid}/split/stream`)
                                  source.onmessage = (e) => {
                                    try {
                                      const data = JSON.parse(e.data) as {
                                        progress?: number
                                        stage?: string
                                        done?: number | null
                                        total?: number | null
                                        source_deleted?: boolean
                                      }
                                      setSplitProgress(data.progress ?? 0)
                                      setSplitStage(data.stage ?? null)
                                      setSplitDone(data.done ?? null)
                                      setSplitTotal(data.total ?? null)
                                      if (data.stage === "finished") {
                                        source.close()
                                        setSplitCreated(data.done ?? null)
                                        setSplitStatus(null)
                                        queryClient.invalidateQueries({ queryKey: ["archives"] })
                                        queryClient.invalidateQueries({ queryKey: ["archive"] })
                                        queryClient.invalidateQueries({ queryKey: ["archive-split-suggestion"] })
                                        // The server only deletes the source when every volume was
                                        // created and the user asked for it, and it says so in
                                        // this frame: a page whose archive is gone has nothing
                                        // left to show, so go back to the library. Anything else
                                        // (a partial run, the source kept) stays put.
                                        if (data.source_deleted === true) {
                                          navigate("/")
                                        }
                                      }
                                    } catch {
                                      setSplitStatus(e.data)
                                    }
                                  }
                                  // The server drops the channel when the job ends, which the
                                  // browser reports as an error even on a clean finish.
                                  source.onerror = () => {
                                    source.close()
                                    setSplitStatus((current) => current ?? t("reader.splitFinishedOrNotFound") ?? null)
                                  }
                                })
                            }}
                          />
                        </div>
                        {splitProgress !== null && (
                          <div style={{ marginTop: 12 }} className="split-progress">
                            <div className="split-progress-track">
                              <div
                                className="split-progress-fill"
                                style={{ width: `${Math.max(0, Math.min(100, Math.round(splitProgress * 100)))}%` }}
                              />
                            </div>
                            <div className="split-progress-label">
                              {t(`reader.splitStage.${splitStage ?? "started"}`)}
                              {splitDone !== null && splitTotal !== null ? ` ${splitDone} / ${splitTotal}` : ""}
                              {` · ${Math.round(splitProgress * 100)}%`}
                            </div>
                          </div>
                        )}
                        {splitCreated !== null && (
                          <div style={{ marginTop: 8, textAlign: "center", fontWeight: 600 }}>
                            {t("reader.splitCreatedVolumes", { count: splitCreated })}
                          </div>
                        )}
                        {splitCreated !== null && splitTotal !== null && splitCreated < splitTotal && (
                          <div className="split-progress-warning">
                            {t("reader.splitPartialFailure", { done: splitCreated, total: splitTotal })}
                          </div>
                        )}
                        {splitStatus && <div style={{ marginTop: 8, textAlign: "center" }}>{splitStatus}</div>}
                      </>
                    ) : (
                      <>
                        <div style={{ textAlign: "center" }}>{t("reader.noArchiveSplitSuggestion")}</div>
                        <div style={{ marginTop: 4, opacity: 0.7, textAlign: "center" }}>
                          {t("reader.archiveSplitAutoAnalyzed")}
                        </div>
                      </>
                    )}
                  </Modal>
                )}
                <h2>{t("categories.categories")}</h2>
                <style>{`
                  .category-chip {
                    border-radius: 5px;
                    overflow: hidden;
                    position: relative;
                  }
                  @keyframes category-chip-sheen-flow {
                    from { background-position: 0 0; }
                    to { background-position: -48px 0; }
                  }
                  .category-chip::before {
                    content: "";
                    position: absolute;
                    inset: 0;
                    background: repeating-linear-gradient(
                      100deg,
                      rgba(255,255,255,0) 0px,
                      rgba(255,255,255,0.95) 8px,
                      rgba(255,255,255,0) 16px,
                      rgba(255,255,255,0) 48px
                    );
                    mix-blend-mode: overlay;
                    pointer-events: none;
                    z-index: 0;
                  }
                  .category-chip:hover::before {
                    animation: category-chip-sheen-flow 2.2s linear infinite;
                  }
                  .category-chip > * {
                    position: relative;
                    z-index: 1;
                  }
                  .category-chip-remove {
                    transition: background-color 0.1s;
                  }
                  .category-chip-remove:hover {
                    background-color: rgba(0,0,0,0.12);
                  }
                `}</style>
                <div style={{ display: "inline-block" }}>
                  {archiveCategories.map((c) => (
                    <div
                      key={c.id}
                      className="gt category-chip"
                      style={{ fontSize: 14, height: 26, padding: 0, display: "inline-flex", alignItems: "stretch", gap: 0, lineHeight: 1 }}
                    >
                      <span className="label" style={{ padding: "0 6px 0 8px", display: "flex", alignItems: "center" }}>
                        {c.name}
                      </span>
                      <a
                        href="#"
                        className="category-chip-remove"
                        style={{
                          textDecoration: "none",
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                          padding: "0 8px",
                          fontSize: "1.3em",
                          borderLeft: "1px solid currentColor",
                          opacity: 0.9,
                        }}
                        onClick={(e) => {
                          e.preventDefault()
                          void removeFromCategory(c.id)
                        }}
                      >
                        ×
                      </a>
                    </div>
                  ))}
                </div>

                <br />
                <span>{t("reader.addTo")}</span>
                <div style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
                  <Select
                    id="category"
                    value=""
                    onValueChange={(v) => {
                      if (v) void addToCategory(v)
                    }}
                    items={[
                      { value: "", label: t("common.NoCategory") },
                      ...staticCategories.map((c) => ({ value: c.id, label: c.name })),
                    ]}
                    size="md"
                    style={{ width: 200, marginTop: 0, marginBottom: 0 }}
                  />
                  <Tooltip label={t("common.newCategory") ?? undefined}>
                    <IconButton
                      variant="favtag-btn"
                      icon={<FaPlus />}
                      size={25}
                      style={{ margin: 0 }}
                      onClick={() => void handleNewCategory()}
                      aria-label={t("common.newCategory") ?? undefined}
                    />
                  </Tooltip>
                </div>

                <h2>{t("reader.rating")}</h2>
                <div style={{ display: "inline-flex", alignItems: "center", gap: 10 }}>
                  <RatingWidget
                    archiveId={archive.arcid}
                    tags={archive.tags}
                    onChange={isTank ? (nextTags) => updateTankoubon.mutate({ metadata: { tags: nextTags } }) : undefined}
                  />
                  {onShare && (
                    <Tooltip label={t("share.action.share")}>
                      <IconButton
                        variant="favtag-btn"
                        icon={<FaShareAlt />}
                        size={25}
                        style={{ margin: 0 }}
                        onClick={onShare}
                        aria-label={t("share.action.share")}
                      />
                    </Tooltip>
                  )}
                </div>
              </div>
            )}
          </div>

          <TagsTable tags={archive.tags} />
        </div>

        <br />
        <br />

        <div className="overlay-bar">
          <div className="overlay-bar-left" style={{ paddingLeft: 0 }}>
            {stampedPageSet.size > 0 && (
              <a
                className={`fas fa-stamp${filterStamped ? " toggled" : ""}`}
                id="filter-stamped"
                href="#"
                style={{ padding: 8, fontSize: 14 }}
                title={t("reader.filterStampedPages") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setFilterStamped((v) => !v)
                }}
              />
            )}
          </div>
          <h2 className="ih">{chapters ? t("reader.chapters") : t("reader.pages")}</h2>
          <div className="chapter-selector" style={{ paddingRight: 0 }}>
            {chapters && (
              <select
                id="chapter-select"
                className="favtag-btn"
                style={{ width: 200 }}
                onChange={(e) => {
                  const page = Number(e.target.value)
                  if (page > 0) onSelectPage(page)
                }}
              >
                {chapters.map((c) => (
                  <option key={c.page} value={c.page}>
                    {displayTocName(c.name, t)}
                  </option>
                ))}
              </select>
            )}
            {loggedIn && chapters && currentChapter && !currentChapter.synthetic && (
              <>
                <a
                  className="fas fa-pencil-alt edit-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14, position: "relative", top: 6 }}
                  title={t("common.editChapterName") ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    setEditTocMenuAt(e.currentTarget.getBoundingClientRect())
                  }}
                />
                {editTocMenuAt && (
                  <ChapterActionMenu
                    mode="edit"
                    anchor={editTocMenuAt}
                    chapters={chapters}
                    onPick={(entry) => void handleEditToc(entry)}
                    onClose={() => setEditTocMenuAt(null)}
                  />
                )}
                <a
                  className="fas fa-trash-alt remove-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14, position: "relative", top: 6 }}
                  title={t("common.deleteChapter") ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    setRemoveTocMenuAt(e.currentTarget.getBoundingClientRect())
                  }}
                />
                {removeTocMenuAt && (
                  <ChapterActionMenu
                    mode="delete"
                    anchor={removeTocMenuAt}
                    chapters={chapters}
                    onPick={(entry) => void handleRemoveToc(entry)}
                    onClose={() => setRemoveTocMenuAt(null)}
                  />
                )}
              </>
            )}
          </div>
        </div>

        <div id="pages-section" className="overview-page-grid" ref={setPageGridEl}>
          {loadedTotal > 0 && loadedStart > 0 && (
            <div style={{ width: "100%", height: 3, background: "rgba(128,128,128,0.15)", marginBottom: 4 }}>
              <div
                style={{
                  height: "100%",
                  width: `${Math.min(100, ((loadedEnd - loadedStart + 1) / loadedTotal) * 100)}%`,
                  marginLeft: `${((loadedStart - 1) / loadedTotal) * 100}%`,
                  background: "rgba(128,128,128,0.4)",
                  borderRadius: 2,
                  transition: "width 0.2s, margin-left 0.2s",
                }}
              />
            </div>
          )}
          {Array.from({ length: loadedTotal || pageCount || 0 }, (_, i) => i + 1).map((page) => {
            const meta = pageMeta.get(page)
            if (meta) {
              const isStamped = stampedPageSet.has(String(page))
              if (filterStamped && !isStamped) return null
              return (
                <PageGridCell
                  key={page}
                  page={page}
                  isStamped={isStamped}
                  loggedIn={loggedIn}
                  highlighted={page === highlightedPage}
                  thumbnailSrc={`/api/archives/${meta.arcId}/thumbnail?page=${meta.localPage}`}
                  onSelectPage={onSelectPage}
                  onSetThumbnail={handleSetThumbnail}
                  onAddToc={handleAddToc}
                  onQuickAddToc={handleQuickAddToc}
                  onOpenLightbox={setLightboxPage}
                  isTank={isTank}
                  isPatch={patchPageSet?.has(page) ?? false}
                  chapterName={chapterNameByPage.get(page)}
                  toc={archive.toc}
                  cornerButtons={supportsHover}
                />
              )
            }
            return (
              <PagePlaceholder
                key={page}
                page={page}
                onVisible={() => {
                  if (page < loadedStart) loadUp()
                  else loadDown()
                }}
              />
            )
          })}
        </div>
        <ScrollToTopFab onJump={() => setScrollAnchor(1)} />
      </div>

      {archiveDeleteOpen && (
        <DeleteConfirmDialog
          isTank={false}
          source={sourceTagFrom(archive.tags)}
          onCancel={() => setArchiveDeleteOpen(false)}
          onConfirm={(blockResubscribe) => void confirmArchiveDelete(blockResubscribe)}
        />
      )}
      {tankDeleteOpen && (
        <TankDeleteChoiceDialog
          title={archive.title}
          onConfirm={(mode) => {
            setTankDeleteOpen(false);
            void (async () => {
              await deleteTankoubon.mutateAsync({ id: archive.arcid, mode });
              navigate(routes.library());
            })();
          }}
          onCancel={() => setTankDeleteOpen(false)}
        />
      )}

      {lightboxPage !== null && (
        <PageLightbox
          archiveId={archive.arcid}
          initialPage={lightboxPage}
          toc={archive.toc}
          loggedIn={loggedIn}
          onQuickAddToc={handleQuickAddToc}
          onEditToc={(entry) => void handleEditToc(entry)}
          onRemoveToc={(entry) => void handleRemoveToc(entry)}
          onClose={() => setLightboxPage(null)}
          pagesOverride={isTank ? tankPages : undefined}
          resolvePage={resolvePage}
        />
      )}
    </>
  )
}
