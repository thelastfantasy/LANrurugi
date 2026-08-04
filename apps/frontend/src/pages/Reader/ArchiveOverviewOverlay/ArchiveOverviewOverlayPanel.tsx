import { useQueryClient } from "@tanstack/react-query"
import type { MouseEvent } from "react"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import {
  useAddTocEntry,
  useAddTocEntryForId,
  useCreateCategory,
  useDeleteTankoubon,
  useRemoveTocEntry,
  useRemoveTocEntryForId,
  useSetArchiveThumbnail,
  useSetTankoubonThumbnail,
  useStampedPages,
  useStampedPagesForArchives,
  useUpdateTankoubon,
} from "../../../api/hooks"
import type { ArchiveMetadata, CategoryMetadata } from "../../../api/types"
import { RatingWidget } from "../../../components/RatingWidget"
import { Tooltip } from "../../../components/Tooltip"
import { confirmDialog, newCategoryDialog, promptDialog } from "../../../dialog"
import { displayTocName, isReservedTocIdentifier } from "../../../lib/tocValidation"
import { routes } from "../../../routes"
import { toast } from "../../../toast"
import { isTankoubonId } from "../../Library/isTankoubonId"
import type { TankoubonChapter } from "../useTankoubonReading"
import { PageGridCell } from "./PageGridCell"
import { PageLightbox } from "./PageLightbox"
import { ChapterActionMenu } from "./shared"
import { TagsTable } from "./TagsTable"

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
  tankPages?: string[]
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const createCategory = useCreateCategory()
  const staticCategories = (categories ?? []).filter((c) => !c.search)
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
        toast({ text: t("Successfully set page {{n}} as the thumbnail!", { n: page }) ?? undefined })
      },
      onError: () => toast({ text: t("Error updating thumbnail") ?? undefined, icon: "error" }),
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
    let message = t("Enter a title for this chapter/section:") ?? ""
    let value = defaultValue
    for (;;) {
      const input = await promptDialog(message, value)
      if (input === null) return null
      const trimmed = input.trim()
      if (trimmed === "") return null
      if (isReservedTocIdentifier(trimmed)) {
        message = t('"{{value}}" is a reserved identifier and can\'t be used as a chapter title. Enter a title for this chapter/section:', { value: trimmed }) ?? ""
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
    const onError = () => toast({ text: t("Error adding/removing chapter:") ?? undefined, icon: "error" })
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
    const onError = () => toast({ text: t("Error adding/removing chapter:") ?? undefined, icon: "error" })
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
    const onError = () => toast({ text: t("Error adding/removing chapter:") ?? undefined, icon: "error" })
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
    if (!(await confirmDialog(t('Are you sure you want to delete "{{name}}"?', { name: entry.name }) ?? ""))) return
    const target = resolve(entry.page)
    if (!target) return
    const onError = () => toast({ text: t("Error adding/removing chapter:") ?? undefined, icon: "error" })
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
      toast({ heading: t("Error modifying category") ?? undefined, icon: "error" })
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
            "Are you sure you want to delete this tankoubon? The archives will remain in your library but will no longer be grouped.",
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
        t("This will delete both metadata and matching files from your system! Please use with caution.") ?? "",
      ))
    ) {
      return
    }
    await fetch(`/api/archives/${archive.arcid}`, { method: "DELETE" })
    navigate(routes.library())
  }

  const pageCount = archive.pagecount

  // Scrolls to and briefly outlines the current page's own thumbnail once, right after the
  // overlay opens from a real click (`autoFocus`, see this component's own prop docs) — otherwise
  // the reader has to hunt for it by eye across a grid that can run into the hundreds of cells for
  // a long archive, with no indication at all of where "here" is. Additive; legacy's own
  // `#archivePagesOverlay` has no equivalent (it opens already scrolled to the top, same as this
  // port without this effect). Skipped entirely when `showOverlayByDefault` auto-opened this
  // overlay instead (`autoFocus` false) — auto-scrolling on every single page load in addition to
  // auto-opening was a real, reported annoyance, even though auto-opening itself is intentional.
  const [highlightedPage, setHighlightedPage] = useState<number | null>(null)
  useEffect(() => {
    if (!autoFocus) return
    // Deferred a tick rather than calling `setHighlightedPage` synchronously in the effect body
    // (the project's own lint rules flag that as cascading-render-prone) — also conveniently lets
    // the just-mounted grid finish its first paint before `scrollIntoView` runs against it.
    const startTimer = setTimeout(() => {
      const cell = document.querySelector(`[data-page-cell="${currentPage}"]`)
      if (!cell) return
      cell.scrollIntoView({ block: "center" })
      setHighlightedPage(currentPage)
    }, 0)
    const clearTimer = setTimeout(() => setHighlightedPage(null), 3000)
    return () => {
      clearTimeout(startTimer)
      clearTimeout(clearTimer)
    }
    // Intentionally empty deps — this is a one-time "where am I" cue for whichever page the
    // overlay opened on, not something that should re-trigger on every `currentPage` change while
    // it stays open (e.g. from clicking around the grid itself).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — legacy's own JS explicitly shows it
          (`fadeTo`) when opening an overlay rather than relying on presence in the DOM, so this
          needs the same explicit override or clicking it (or even seeing it) does nothing. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: "block", opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay">
        <h2 className="ih" style={{ textAlign: "center" }}>
          {t("Archive Overview")}
        </h2>

        <div id="tagContainer" className="caption caption-tags caption-reader">
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
                <h2>{t("Admin Options")}</h2>

                <input
                  className="stdbtn"
                  type="button"
                  value={(isTank ? t("Edit Tankoubon") : t("Edit Archive Metadata")) ?? undefined}
                  onClick={() => navigate(isTank ? routes.tankoubonEdit(archive.arcid) : routes.edit(archive.arcid))}
                />
                <input
                  className="stdbtn"
                  type="button"
                  value={(isTank ? t("Delete Tankoubon") : t("Delete Archive")) ?? undefined}
                  onClick={() => void deleteArchive()}
                />
                <br />

                <h2>{t("Categories")}</h2>
                <div style={{ display: "inline-block" }}>
                  {archiveCategories.map((c) => (
                    <div key={c.id} className="gt" style={{ fontSize: 14, padding: 4 }}>
                      <span className="label">{c.name}</span>
                      <a
                        href="#"
                        style={{ marginLeft: 4, marginRight: 2 }}
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
                <span>{t("Add to : ")}</span>
                <select
                  id="category"
                  className="favtag-btn"
                  style={{ width: 200 }}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void addToCategory(e.target.value)
                  }}
                >
                  <option value="">{t(" -- No Category -- ")}</option>
                  {staticCategories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>
                <Tooltip label={t("New Category") ?? undefined}>
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

                <h2>{t("Rating")}</h2>
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
          <div className="overlay-bar-left">
            {stampedPageSet.size > 0 && (
              <a
                className={`fas fa-stamp${filterStamped ? " toggled" : ""}`}
                id="filter-stamped"
                href="#"
                style={{ padding: 8, fontSize: 14 }}
                title={t("Filter stamped pages") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setFilterStamped((v) => !v)
                }}
              />
            )}
          </div>
          <h2 className="ih">{chapters ? t("Chapters") : t("Pages")}</h2>
          <div className="chapter-selector">
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
            {loggedIn && chapters && currentChapter && (
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
                  title={t("Edit Chapter name") ?? undefined}
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
                  title={t("Delete Chapter") ?? undefined}
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

        <div id="pages-section" style={{ textAlign: "center" }}>
          {Array.from({ length: pageCount }, (_, i) => i + 1).map((page) => {
            const isStamped = stampedPageSet.has(String(page))
            if (filterStamped && !isStamped) return null
            const target = resolve(page)
            if (!target) return null
            return (
              <PageGridCell
                key={page}
                page={page}
                isStamped={isStamped}
                loggedIn={loggedIn}
                highlighted={page === highlightedPage}
                thumbnailSrc={`/api/archives/${target.arcId}/thumbnail?page=${target.localPage}`}
                onSelectPage={onSelectPage}
                onSetThumbnail={handleSetThumbnail}
                onAddToc={handleAddToc}
                onQuickAddToc={handleQuickAddToc}
                onOpenLightbox={setLightboxPage}
              />
            )
          })}
        </div>
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
