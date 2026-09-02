import { useQueryClient } from "@tanstack/react-query"
import type { MouseEvent } from "react"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import {
  useAddTocEntry,
  useAddTocEntryForId,
  useArchivePages,
  useCreateCategory,
  useDeletePatch,
  useDeleteTankoubon,
  useRemoveTocEntry,
  useRemoveTocEntryForId,
  useSetArchiveThumbnail,
  useSetTankoubonThumbnail,
  useStampedPages,
  useStampedPagesForArchives,
  useUpdateTankoubon,
} from "@/api/hooks"
import type { ArchiveMetadata, ArchivePage, CategoryMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { RatingWidget } from "@/components/common-ui/Form"
import { confirmDialog, newCategoryDialog, promptDialog } from "@/dialog"
import { usePaginatedOverview } from "@/hooks/usePaginatedOverview"
import type { TankoubonChapter } from "@/hooks/useTankoubonReading"
import { routes } from "@/lib/routes"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"
import { sortCategories } from "@/lib/utils/sortCategories"
import { displayTocName, isReservedTocIdentifier } from "@/lib/utils/tocValidation"
import { toast } from "@/toast"

import { PageGridCell } from "./PageGridCell"
import { PageLightbox } from "./PageLightbox"
import { ChapterActionMenu } from "./shared"
import { TagsTable } from "./TagsTable"

/** Scroll-to-top floating button, visible only after scrolling down 300px. */
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

/** Placeholder for an unloaded page in the grid so the scrollbar reflects the true total. */
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
      className="id3 quick-thumbnail"
      style={{ width: 205, aspectRatio: "1 / 1.414", background: "rgba(128,128,128,0.06)", borderRadius: 4 }}
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
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()
  const staticCategories = sortCategories((categories ?? []).filter((c) => !c.search))
  const archiveCategories = staticCategories.filter((c) => c.archives.includes(archive.arcid))
  const isTank = isTankoubonId(archive.arcid)

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

  const setThumbnail = useSetArchiveThumbnail(isTank ? "" : archive.arcid)
  const setTankoubonThumbnail = useSetTankoubonThumbnail(archive.arcid)
  const addTocEntry = useAddTocEntry(isTank ? "" : archive.arcid)
  const removeTocEntry = useRemoveTocEntry(isTank ? "" : archive.arcid)
  const addTocEntryForId = useAddTocEntryForId()
  const removeTocEntryForId = useRemoveTocEntryForId()
  const deleteTankoubon = useDeleteTankoubon()
  const updateTankoubon = useUpdateTankoubon(archive.arcid)

  const [thumbnailVersion, setThumbnailVersion] = useState(0)

  /** Legacy's `.set-thumbnail` handler. In Tankoubon mode always sets the Tankoubon's own cover;
   * the backend resolves the owning member archive. */
  function handleSetThumbnail(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
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
  async function handleAddToc(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
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
      if (
        !(await confirmDialog(
          t(
            "reader.confirmDeleteTankoubon",
          ) ?? "",
        ))
      ) {
        return
      }
      await deleteTankoubon.mutateAsync(archive.arcid)
      navigate(routes.library())
      return
    }
    if (
      !(await confirmDialog(
        t("common.thisWillDeleteBothMetadata") ?? "",
        true,
      ))
    ) {
      return
    }
    await fetch(`/api/archives/${archive.arcid}`, { method: "DELETE" })
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
  const scrolledRef = useRef(false)
  useEffect(() => {
    if (!autoFocus) return
    if (scrolledRef.current) return
    const section = document.getElementById("pages-section")
    if (!section) return
    const obs = new MutationObserver(() => {
      const cell = document.querySelector(`[data-page-cell="${currentPage}"]`)
      if (!cell) return
      cell.scrollIntoView({ block: "center" })
      setHighlightedPage(currentPage)
      scrolledRef.current = true
      obs.disconnect()
    })
    obs.observe(section, { childList: true, subtree: true })
    const tryImmediate = setTimeout(() => {
      const cell = document.querySelector(`[data-page-cell="${currentPage}"]`)
      if (cell) {
        cell.scrollIntoView({ block: "center" })
        setHighlightedPage(currentPage)
        scrolledRef.current = true
        obs.disconnect()
      }
    }, 0)
    const clearTimer = setTimeout(() => setHighlightedPage(null), 3000)
    return () => {
      obs.disconnect()
      clearTimeout(tryImmediate)
      clearTimeout(clearTimer)
    }
  }, [autoFocus, currentPage, paginatedPages])

  return (
    <>
      <div id="overlay-shade" style={{ display: "block", opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay" style={{ padding: "0 16px", boxSizing: "border-box", overscrollBehavior: "contain" }}>
        <h2 className="ih" style={{ textAlign: "center" }}>
          {t("reader.archiveOverview")}
        </h2>

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

                <input
                  className="stdbtn"
                  type="button"
                  style={archive.has_patch ? { width: "auto", minWidth: 110 } : undefined}
                  value={(isTank ? t("common.editTankoubon") : t("reader.editArchiveMetadata")) ?? undefined}
                  onClick={() => navigate(isTank ? routes.tankoubonEdit(archive.arcid) : routes.edit(archive.arcid))}
                />
                <input
                  className="stdbtn"
                  type="button"
                  style={archive.has_patch ? { width: "auto", minWidth: 110 } : undefined}
                  value={(isTank ? t("common.deleteTankoubon") : t("common.deleteArchive")) ?? undefined}
                  onClick={() => void deleteArchive()}
                />
                {!isTank && archive.has_patch && <DeletePatchButton archiveId={archive.arcid} patchPageSet={patchPageSet} />}
                <br />

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
                <select
                  id="category"
                  className="favtag-btn"
                  style={{ width: 200 }}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void addToCategory(e.target.value)
                  }}
                >
                  <option value="">{t("common.NoCategory")}</option>
                  {staticCategories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
                <Tooltip label={t("common.newCategory") ?? undefined}>
                  <a
                    href="#"
                    style={{ marginLeft: 6 }}
                    onClick={(e) => {
                      e.preventDefault()
                      void handleNewCategory()
                    }}
                  >
                    <i className="fas fa-plus" />
                  </a>
                </Tooltip>

                <h2>{t("reader.rating")}</h2>
                <RatingWidget
                  archiveId={archive.arcid}
                  tags={archive.tags}
                  onChange={isTank ? (nextTags) => updateTankoubon.mutate({ metadata: { tags: nextTags } }) : undefined}
                />
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

        <div id="pages-section" style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: 8 }}>
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
