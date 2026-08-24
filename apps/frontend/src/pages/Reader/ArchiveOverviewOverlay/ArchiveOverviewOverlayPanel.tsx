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
import { Tooltip } from "@/components/Display"
import { RatingWidget } from "@/components/Form"
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

/**
 * Scroll-boundary sentinel — triggers `onVisible` when the user scrolls within
 * `threshold` px of this element. Uses the overlay's own scroll container
 * (`#i1`) rather than IntersectionObserver so the initial scroll-to-current-page
 * doesn't falsely trigger a cascade of preloads.
 */
/** Scroll-to-top floating button — absolute within the modal (`.id1` is the positioning
 * parent since it has `position: fixed`), visible only after scrolling down 300px. */
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

/** Placeholder for an unloaded page in the overview grid — same dimensions as a
 * real `PageGridCell` so the scrollbar always reflects the true total. Triggers
 * `onVisible` via IntersectionObserver when scrolled near. */
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
    // Group consecutive pages into ranges
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

// Mirrors legacy's `#archivePagesOverlay` (`updateArchiveOverlay`/`generateThumbnails` in
// `~/LANraragi/public/js/reader.js`) — thumbnail (left) + Admin Options/Categories/Rating (right)
// side by side via `.reader-thumbnail`'s `display:inline-block` (verified against
// `~/LANraragi/public/css/lrr.css`), the full per-namespace tags table below it, then a thumbnail
// grid scoped to the current chapter (or the whole archive if there's no TOC).
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
  /** Scroll/briefly-highlight the current page's own thumbnail right after this overlay mounts
   * (see the effect below) — only meaningful for a real user click on the grid-toggle button;
   * `false` (the default) when `Reader.tsx`'s own `showOverlayByDefault` setting is what opened
   * this overlay instead, so a fresh page load doesn't also yank the scroll position on top of
   * auto-opening. */
  autoFocus?: boolean
  /** Only passed when `archive` is actually the synthetic multi-archive object
   * `useTankoubonReading` builds (`archive.arcid` is then the Tankoubon's own id, not a real
   * archive) — resolves one of `archive`'s own *global* page numbers back to the real member
   * archive (and that archive's own *local* page number) it actually belongs to. Every per-page
   * action below (thumbnail URLs, "set as cover", ToC add/remove) needs this instead of using
   * `archive.arcid`/the raw page number directly, since ToC/thumbnails are still real per-archive
   * resources even when reading the Tankoubon as one concatenated book — legacy's own
   * `getArchiveForPage` is used the exact same way throughout `reader_archive_overlay.js`.
   * `undefined` in the plain single-archive case, where every callback below already falls back
   * to `{ arcId: archive.arcid, localPage: page }` (the identity mapping — unchanged behavior). */
  resolvePage?: (globalPage: number) => { arcId: string; localPage: number } | null
  /** Same Tankoubon-mode-only condition as `resolvePage` — needed separately (not derivable from
   * `resolvePage` alone) for the "filter stamped pages" toggle, which has to fetch every member
   * archive's own stamped-page list up front to build the filter, not resolve one page at a time. */
  tankChapters?: TankoubonChapter[]
  /** Tankoubon-mode only: the already-concatenated multi-archive page list
   * (`useTankoubonReading`'s own `pages.data.pages`) — passed straight through to `PageLightbox`'s
   * own `pagesOverride` so its large preview doesn't re-fetch `archive.arcid`'s own pages (wrong;
   * that id is the Tankoubon's, not a real archive's). */
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

  // Legacy's `#filter-stamped` (`reader.js`'s `checkStampedPages`/`filterStampedOverlay`) — marks
  // each thumbnail `data-stamped=true` if `GET /archives/{id}/stamps` includes its page number,
  // then a toggle hides every non-stamped thumbnail so the grid becomes a stamped-pages-only view.
  // In Tankoubon mode this has to merge stamped-page lists across every member archive, each
  // converted from that archive's own local page numbers back into the Tankoubon's global
  // numbering via its own chapter's `startPage` — matches legacy's own tank-mode
  // `checkStampedPages` doing the same conversion the other direction (`getArchiveForPage`).
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

  // Legacy's `getCurrentChapter` (`reader.js`) — the last ToC entry whose `startPage` is `<=` the
  // reader's current page; only leaf chapters (this port has no sub-chapter nesting) get
  // edit/delete icons (legacy: `currentChapter.chapters === null`).
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

  // `useSetArchiveThumbnail`'s own `onSuccess` invalidates the *metadata* query, but the cover
  // `<img>` below points at a plain, param-free `/api/archives/{id}/thumbnail` URL — a browser
  // caches an image response by URL alone, so a same-URL re-render after a successful "set as
  // thumbnail" click kept serving the old cached bytes instead of the just-regenerated ones (only
  // a full page reload, which bypasses the image cache incidentally rather than by design, ever
  // showed the update). Bumped on success and appended as a cache-busting query param below.
  // Legacy itself has no equivalent fix — its own `.set-thumbnail` handler (`reader.js`) never
  // re-fetches the cover `<img>` at all after a successful PUT, so the same staleness exists
  // there too (confirmed by reading that handler's full body — it only ever calls `Server.callAPI`
  // and shows a toast, nothing image-related) — this is a straightforward improvement, not a port
  // of some real legacy mechanism.
  const [thumbnailVersion, setThumbnailVersion] = useState(0)

  // Legacy's `.set-thumbnail` click handler (`reader.js`) — regenerates the cover thumbnail from
  // this page and shows a toast; `e.stopPropagation()` so the click doesn't also trigger the
  // thumbnail's own `onSelectPage` navigation. In Tankoubon mode this always sets the Tankoubon's
  // *own* cover (never a specific member archive's) from `page` interpreted as a global page
  // number — the backend resolves which member archive that actually falls in itself
  // (`update_tankoubon_thumbnail`), matching legacy's own tank-mode `.set-thumbnail` handler
  // (`reader_archive_overlay.js`).
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

  // Shared by `handleAddToc`/`handleEditToc`'s manual-entry flow — loops the same prompt (with an
  // error prefixed onto the message) until the user either cancels or enters something that isn't
  // a bare internal-identifier-style string (`c1`-`c15`/`toc`, case-insensitive). These are never
  // actually stored anywhere (the backend only ever sees real display text), but a user typing the
  // identifier itself rather than real chapter text is almost certainly confused about what the
  // field expects, not intentionally naming a chapter "c1" — real display text like "目录"/"第 1
  // 章" is left completely unrestricted, matching legacy's own free-text ToC title storage.
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

  // Legacy's `.add-toc` click handler + `addTocSection` (`reader.js`) — prompts for a chapter
  // title, then PUTs the new ToC entry. Empty/cancelled input adds nothing (matches legacy's own
  // `result.value.trim() !== ""` guard). `page` is a *global* page number in Tankoubon mode; ToC
  // entries are still real per-archive data even when reading a Tankoubon as one concatenated
  // book (matches legacy's own `addTocSection`'s `getArchiveForPage(page)` call), so this resolves
  // the real owning archive + that archive's own local page number before writing.
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

  // Right-click on the same "add chapter" icon (see `handleAddToc` above for its plain left-click
  // prompt-based flow) — a purely additive shortcut (no legacy equivalent) for the handful of
  // chapter titles that come up often enough in real doujin/manga scans to not need typing out
  // every time: 封面/封底/目录/彩页, plus 第N章 (1–15) via a `<select>`. Submits immediately on
  // pick — no separate confirm step, matching this popover's own single-click-and-done feel
  // rather than a form the user has to explicitly submit.
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

  // Legacy's `.edit-toc` click handler (`reader.js`: `addTocSection(currentChapter.startPage,
  // currentChapter.name)`) — re-prompts with the existing name pre-filled as a placeholder, then
  // re-adds the entry at the same page (the host's `add_toc_entry` replaces same-page entries
  // rather than duplicating them, matching legacy's own upsert-by-page semantics).
  //
  // Takes an explicit `entry` (defaulting to `currentChapter`, matching legacy's own real
  // limitation of only ever editing whichever chapter the reader is currently scrolled into) —
  // the `ChapterActionMenu`-driven callers below pass whichever entry was actually picked from
  // the "edit chapter" menu instead, the same real improvement over legacy's single-target
  // restriction `handleRemoveToc`'s own docs describe for delete.
  //
  // Refuses to edit a reserved preset identifier (`c1`-`c20`/`toc`) at all — `ChapterActionMenu`
  // already disables picking one of these in its own list (see that component's own docs), but
  // this plain left-click path (which always targets `currentChapter`, bypassing that menu
  // entirely) needs the same guard independently. The whole point of storing an identifier
  // instead of real text is that the two are deliberately different — silently "editing" one into
  // free text here would erase that distinction without the user necessarily intending to change
  // what kind of entry it is, not just its wording. Changing one of these is done by deleting it
  // and re-applying a (possibly different) preset instead.
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

  // Legacy's own `.remove-toc` (`reader.js`'s `removeTocSection`) only ever targets
  // `getCurrentChapter()` — whichever chapter the reader happens to be scrolled into right now —
  // with no way to pick a different one at all; a real, confirmed limitation in legacy itself,
  // not a porting gap (verified against `reader.js:157-158,1702` — `getCurrentChapter` really is
  // the only chapter `.edit-toc`/`.remove-toc` ever operate on). A real improvement over that: the
  // delete button now opens `ChapterActionMenu` listing every chapter in the archive, so deleting one
  // that isn't the currently-viewed one doesn't require first scrolling/navigating to it.
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

  // Deleting a Tankoubon removes the grouping record itself, not any of its member archives'
  // files (matches legacy's own `Server.deleteTankoubon` vs `Server.deleteArchive` split in
  // `reader_archive_overlay.js`) — a different, much less permanent operation than deleting a
  // real archive, so it gets its own (milder) confirmation copy rather than reusing the
  // file-deletion warning below verbatim.
  async function deleteArchive() {
    if (isTank) {
      // Existing translation key (already in every locale file, previously unused anywhere in the
      // app) — more accurate than a generic "are you sure" for this operation specifically, since
      // it clarifies member archives themselves aren't touched, unlike the real-file-deletion
      // warning below.
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

  // Overridden by the scroll-to-top FAB (`ScrollToTopFab`'s own `onJump`) — re-anchors the
  // paginated fetch to page 1 instead of just scrolling, so the pages skipped over by the jump
  // stay unloaded (placeholders) rather than every one of them firing `onVisible` at once when
  // they all land in the viewport simultaneously (a real observed bug: a jump from page ~450 to
  // the top fired dozens of concurrent `loadUp`/`loadDown` calls in the same render, one per
  // placeholder that happened to be in view right after the jump).
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

  // Patch page detection — `is_patch` from `/archives/{id}/files` (now per-page) drives a
  // distinct background color on the overview grid's thumbnail cells so the user can tell at a
  // glance which pages are original and which came from a `.patch.zip`.
  const singlePagesForPatch = useArchivePages(isTank ? null : archive.arcid)
  const patchPageSet = (() => {
    const list = isTank ? tankPages : singlePagesForPatch.data?.pages
    if (!list) return null as Set<number> | null
    const s = new Set<number>()
    list.forEach((p, i) => { if (p.is_patch) s.add(i + 1) })
    return s
  })()

  // Scrolls to and briefly outlines the current page's own thumbnail once, right after the
  // overlay opens from a real click (`autoFocus`, see this component's own prop docs) — otherwise
  // the reader has to hunt for it by eye across a grid that can run into the hundreds of cells for
  // a long archive, with no indication at all of where "here" is. Additive; legacy's own
  // `#archivePagesOverlay` has no equivalent (it opens already scrolled to the top, same as this
  // port without this effect). Skipped entirely when `showOverlayByDefault` auto-opened this
  // overlay instead (`autoFocus` false) — auto-scrolling on every single page load in addition to
  // auto-opening was a real, reported annoyance, even though auto-opening itself is intentional.
  const [highlightedPage, setHighlightedPage] = useState<number | null>(null)
  const scrolledRef = useRef(false)
  useEffect(() => {
    if (!autoFocus) return
    if (scrolledRef.current) return
    const section = document.getElementById("pages-section")
    if (!section) return
    // With paginated loading the target cell may not exist yet — watch for it.
    const obs = new MutationObserver(() => {
      const cell = document.querySelector(`[data-page-cell="${currentPage}"]`)
      if (!cell) return
      cell.scrollIntoView({ block: "center" })
      setHighlightedPage(currentPage)
      scrolledRef.current = true
      obs.disconnect()
    })
    obs.observe(section, { childList: true, subtree: true })
    // Also try immediately in case the cell is already there (deferred so eslint
    // set-state-in-effect doesn't flag the synchronous setHighlightedPage call).
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
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — legacy's own JS explicitly shows it
          (`fadeTo`) when opening an overlay rather than relying on presence in the DOM, so this
          needs the same explicit override or clicking it (or even seeing it) does nothing. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: "block", opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay" style={{ padding: "0 16px", boxSizing: "border-box", overscrollBehavior: "contain" }}>
        <h2 className="ih" style={{ textAlign: "center" }}>
          {t("reader.archiveOverview")}
        </h2>

        <div id="tagContainer" className="caption caption-tags caption-reader" style={{ maxWidth: "100%", boxSizing: "border-box", overflow: "hidden" }}>
          <br />
          <div style={{ marginBottom: 16 }}>
            {/* Legacy's own `.id3 img { max-height: 275px }` alone doesn't keep this narrow — a
                landscape-oriented cover (a raw panel image rather than a proper portrait cover,
                confirmed via a real archive that reproduces this) has plenty of headroom under
                that height cap to still render very wide, pushing Admin Options below instead of
                beside it. Legacy avoids this because `#archivePagesOverlay` itself carries `.id1`
                (`width: 228px`), which `.id3.nocrop img { max-width: 95% }` computes against —
                this port's own `#tagContainer` (`.caption-reader { min-width: 50% }`) has no such
                fixed width to inherit from, so the same 95%-of-ancestor rule alone doesn't
                reliably leave room for Admin Options beside it. 200px lands close to legacy's own
                effective ~217px (95% of 228px) without depending on an ancestor width this port
                doesn't have. */}
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
                {/* Scoped hover style for the remove-category `×` — inline styles can't
                    express `:hover`, and this is a small enough one-off to not warrant a
                    dedicated CSS module/class in the shared theme files. */}
                <style>{`
                  .category-chip {
                    border-radius: 5px;
                    overflow: hidden;
                    position: relative;
                  }
                  /* Diagonal sheen — thin light stripes at a 45° angle, the classic flat
                     "brushed metal" Windows-button texture. Static at rest; on hover the
                     stripes slide across the chip (a plain background-position animation,
                     no glow/blur) for the same "hovered = alive" cue a real button gives. */
                  @keyframes category-chip-sheen-flow {
                    from { background-position: 0 0; }
                    to { background-position: -48px 0; }
                  }
                  /* Soft, wide, blurred-edge bands (unlike the earlier hard-edged stripes) so
                     the hover animation reads as liquid flowing sideways rather than a texture
                     ticking past — each band fades in/out via multiple gradient stops instead
                     of a sharp on/off repeating-linear-gradient. */
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
                  // The default `useUpdateArchiveMetadata`-backed persistence PUTs
                  // `/archives/{archiveId}/metadata` — wrong for a Tankoubon, whose tags live on
                  // its own `/tankoubons/{id}` record instead (`RatingWidget`'s own `onChange`
                  // prop exists specifically for this — same override the Library context menu's
                  // tankoubon rows already use).
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
                {/* Left-click opens the same "pick any chapter" menu the delete icon already
                    has, rather than legacy's own left-click-edits-`currentChapter`-directly
                    shortcut — the plain-click path silently no-ops on a reserved preset
                    identifier now (see `handleEditToc`'s own guard), which reads as "broken" with
                    no explanation if it's still the default click behavior instead of routing
                    through the menu, where that same entry shows up visibly disabled instead. */}
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
          {/* Progress bar */}
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
          {/* Render ALL pages — loaded = real cell, unloaded = placeholder.
              Placeholder cells give the scrollbar its true height immediately,
              so sentinels only fire when the user genuinely scrolls there. */}
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
            // Placeholder for unloaded page — same size as a real cell
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
        {/* Scroll-to-top FAB — sticky within the scrollable overlay, shown only when scrolled */}
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
