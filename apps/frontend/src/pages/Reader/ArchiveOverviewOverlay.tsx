import { useQueryClient } from '@tanstack/react-query'
import type { MouseEvent, ReactNode } from 'react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import { useAddTocEntry, useRemoveTocEntry, useSetArchiveThumbnail, useStampedPages } from '../../api/hooks'
import type { ArchiveMetadata, CategoryMetadata } from '../../api/types'
import { PopupMenu, PopupMenuItem } from '../../components/PopupMenu'
import RatingWidget from '../../components/RatingWidget'
import StarRatingDisplay from '../../components/StarRating'
import Tooltip from '../../components/Tooltip'
import { confirmDialog, promptDialog } from '../../dialog'
import { parseRating } from '../../lib/rating'
import { getTagSearchURL } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../../theme'
import { toast } from '../../toast'

// Namespaces treated as timestamps for display (legacy `buildTagsDiv`: `/^(date|time)/.test(key)`
// converts the tag value through a date formatter instead of printing it raw).
const TIMESTAMP_NAMESPACE = /^(date|time)/i

function displayNamespace(key: string): string {
  if (key === 'date_added') return 'Date Added'
  return key.charAt(0).toUpperCase() + key.slice(1)
}

function formatTagValue(namespace: string, value: string): string {
  if (!TIMESTAMP_NAMESPACE.test(namespace)) return value
  const ms = Number(value) * 1000
  if (Number.isNaN(ms)) return value
  return new Date(ms).toLocaleDateString()
}

/** Mirrors legacy's `splitTagsByNamespace` + `buildTagsDiv` (`~/LANraragi/public/js/mod/common.js`)
 * — groups a flat comma-separated tag string by its `namespace:value` prefix (untagged values fall
 * under `other`), rendered as a `caption-namespace` row per namespace with each value as a
 * clickable search-link chip. `rating:` gets its own gold-star rendering instead of the raw tag
 * value (see the `namespace === 'rating'` branch below) — legacy's own real overview page shows
 * the star icons in this table *in addition to* the separate interactive `RatingWidget` above it
 * (confirmed against a real screenshot of a rated archive), so this table must render it too, not
 * skip it. Still a real, working search-link chip underneath, though — legacy's own real rating
 * chip *is* clickable (a real user-confirmed link, e.g. `?q=rating%3A⭐⭐⭐⭐⭐$` against a live
 * legacy instance), a link this port's own `q=rating:2.5$` (the equivalent search against this
 * app's own decimal-encoded storage format — verified live: correctly returns exactly the archive
 * carrying that tag) actually and correctly answers, unlike an earlier version of this component
 * that dropped the link entirely on the assumption nobody would search by star count — wrong,
 * since legacy itself treats it as a completely ordinary searchable tag. No underline on it
 * specifically, though (a real, deliberate deviation, not a bug) — legacy's own underlined
 * rating-star link reads like a broken/dead link at a glance, which the star icons alone don't
 * need to invite. */
function TagsTable({ tags }: { tags: string }) {
  if (!tags) return null
  const byNamespace = new Map<string, string[]>()
  for (const raw of tags.split(',')) {
    const tag = raw.trim()
    if (!tag) continue
    const idx = tag.indexOf(':')
    const namespace = idx === -1 ? 'other' : tag.slice(0, idx).trim()
    const value = idx === -1 ? tag : tag.slice(idx + 1).trim()
    const list = byNamespace.get(namespace) ?? []
    list.push(value)
    byNamespace.set(namespace, list)
  }

  const namespaces = [...byNamespace.keys()].sort()
  if (namespaces.length === 0) return null

  return (
    <table className="itg" style={{ boxShadow: 'none', border: 'none', borderRadius: 0 }}>
      <tbody>
        {namespaces.map((namespace) => (
          <tr key={namespace}>
            <td className={`caption-namespace ${namespace.toLowerCase()}-tag`}>
              {displayNamespace(namespace)}:
            </td>
            <td>
              {namespace.toLowerCase() === 'rating' ? (
                <div className="gt">
                  <a
                    href={getTagSearchURL(namespace, (byNamespace.get(namespace) ?? [])[0] ?? '')}
                    onClick={(e) => e.stopPropagation()}
                    style={{ textDecoration: 'none' }}
                  >
                    <StarRatingDisplay rating={parseRating((byNamespace.get(namespace) ?? [])[0]) ?? 0} size={16} />
                  </a>
                </div>
              ) : (
                (byNamespace.get(namespace) ?? []).map((value) => (
                  <div className="gt" key={value}>
                    {/* `source` is a link to an external, third-party site — real `target="_blank"`
                        so it opens a new tab instead of navigating the reader away, matching
                        `TagTable.tsx`'s own real `source` branch (this table predates that shared
                        component and never got the same split when it landed there; this was a
                        real, independently-discovered bug, not a copy of an already-fixed one). */}
                    {namespace === 'source' ? (
                      <a
                        href={getTagSearchURL(namespace, value)}
                        target="_blank"
                        rel="noreferrer"
                        onClick={(e) => e.stopPropagation()}
                      >
                        {value}
                      </a>
                    ) : (
                      <a href={getTagSearchURL(namespace, value)} onClick={(e) => e.stopPropagation()}>
                        {formatTagValue(namespace, value)}
                      </a>
                    )}
                  </div>
                ))
              )}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}

/** One page thumbnail in the overview grid — shows a spin icon (same `fa-circle-notch fa-spin`
 * class legacy's own equivalent uses) while its `<img>` hasn't loaded yet, removed once it has.
 *
 * Legacy's real equivalent (`reader.js`'s `updateArchiveOverlay`/`generateThumbnails`) instead
 * polls a Minion job's progress notes to know which pages have a generated thumbnail yet, since
 * legacy pre-extracts thumbnails as a separate background step. This app's own
 * `GET /archives/{id}/thumbnail?page=N` has no such split — a cache miss regenerates synchronously
 * and blocks the same request until it's ready (see that handler's own docs,
 * `crates/lanrurugi-api/src/archives.rs`) — so the browser's native `<img>` `onLoad` event already
 * *is* the real "this page's thumbnail is ready" signal, with nothing else to poll.
 *
 * Positioned via real `position: absolute` centering within the parent `.id3.quick-thumbnail`
 * (itself `position: relative` — set by the caller), rather than reusing `Library.tsx`'s
 * `.ttspinner` class as-is: that class's own CSS is a `top: -162px` *relative* offset tuned
 * specifically for `ArchiveCard`'s layout, where a full-size `wait_warmly.jpg` placeholder
 * `<img>` occupies real space immediately before it in DOM flow — this grid's cards have no such
 * placeholder image, so the same fixed offset pushed the icon above the card entirely (confirmed
 * via a real `getBoundingClientRect()` comparison: the spinner's rect landed above the card's own
 * top edge, not inside it).
 *
 * Hides the not-yet-loaded `<img>` with `visibility: hidden` (which still occupies real layout
 * space, so the browser can compute whether it intersects the viewport), never `display: none`
 * (which removes it from layout, and Chrome's real `loading="lazy"` never fires the network
 * request for an image in that state at all — confirmed live: an earlier version of this
 * component that used `display: none` here left every one of a 293-page archive's thumbnails
 * stuck on their spinner forever, `list_network_requests` showing zero `thumbnail?page=N`
 * requests ever fired). `Library.tsx`'s own `ArchiveCard` gets away with `display: none` only
 * because its thumbnail `<img>` was never marked `loading="lazy"` to begin with. */
function OverviewThumbnail({ src, alt }: { src: string; alt: string | undefined }) {
  const [loaded, setLoaded] = useState(false)
  return (
    <>
      {!loaded && (
        // The centering transform lives on this plain, non-animated wrapper, not on the `<i>`
        // itself — `fa-spin`'s own CSS animation drives the icon's `transform` (a rotation) every
        // frame, which silently overwrote a `translate(-50%, -50%)` placed directly on the same
        // element (only one `transform` can apply at a time; they don't compose) and put the
        // icon's rotation pivot at the card's top-left corner instead of centered on it —
        // confirmed live via `getBoundingClientRect()`: the icon's rendered center sat well right
        // of the card's true horizontal center. Splitting the two transforms across parent/child
        // is what lets both apply independently.
        <span
          style={{
            position: 'absolute',
            top: '50%',
            left: '50%',
            transform: 'translate(-50%, -50%)',
          }}
        >
          <i className="fa fa-4x fa-circle-notch fa-spin" aria-hidden="true"></i>
        </span>
      )}
      <img
        loading="lazy"
        alt={alt}
        src={src}
        style={loaded ? undefined : { visibility: 'hidden' }}
        onLoad={() => setLoaded(true)}
      />
    </>
  )
}

/** The "第 N 页" label shown over a page-grid cell — its own component (was previously a plain
 * `<span className="page-number">`, sharing that CSS class with the two hover-reveal buttons
 * below purely because legacy's own markup groups all three under it). Genuinely centered
 * (`left: 50%` + `translateX(-50%)`) rather than legacy's own real `left: 30%` (verified against
 * `lrr.css`) — that value was never actually a deliberate "off-center" design choice to preserve,
 * just an artifact of the label's own text width never being accounted for in a fixed percentage;
 * a real centering rule is what "第 N 页" visibly reads as trying to be. */
function PageNumberLabel({ children }: { children: ReactNode }) {
  return (
    <span
      className="page-number"
      style={{ left: '50%', transform: 'translateX(-50%)' }}
    >
      {children}
    </span>
  )
}

/** One of the two hover-revealed action buttons in a page-grid cell (`SetThumbnailButton`/
 * `AddChapterButton` below) — split out from a shared `page-number` class (legacy's own
 * `reader.js` markup puts the page-number label and both buttons under that one class, since all
 * three want the same `position: absolute` + hidden-until-hovered behavior) into its own
 * component with its own React-driven hover state, once their *positions* stopped actually
 * matching each other (the label is now genuinely centered — see `PageNumberLabel` — while the
 * buttons anchor to a `right`-anchored corner instead). Three unrelated things sharing one CSS
 * class/hover-reveal mechanism just because they used to occupy the same *area* was more coupling
 * than the actual relationship between them warranted once that stopped being true. */
function PageGridActionIcon({
  icon,
  corner,
  title,
  hovered,
  onClick,
  onContextMenu,
}: {
  icon: string
  corner: 'top' | 'bottom'
  title: string | undefined
  /** Lifted to the parent `.quick-thumbnail` cell rather than tracked on this element itself —
   * at rest this icon sits at `z-index: -1`, *behind* the thumbnail `<img>`, so the pointer never
   * actually reaches it to fire its own `onMouseEnter` in the first place (confirmed live: a
   * version of this component with its own local hover state never once revealed itself, since
   * entering it was exactly the thing being behind another element prevented). Legacy's real
   * equivalent (`.quick-thumbnail:hover>.page-number`) sidesteps this the same way, by keying off
   * the *parent's* hover instead of the icon's own. */
  hovered: boolean
  onClick: (e: MouseEvent) => void
  /** Only the "add chapter" icon actually supplies this (see `QuickAddTocPopover`) — additive,
   * no legacy equivalent at all. */
  onContextMenu?: (e: MouseEvent) => void
}) {
  return (
    <a
      href="#"
      title={title}
      className={`fas ${icon}`}
      onClick={onClick}
      onContextMenu={onContextMenu}
      style={{
        position: 'absolute',
        // Legacy's own two real values here are `top: 2%`/`top: 80%` (verified against
        // `reader.js`) — both computed from the top, so the "bottom" button's exact position
        // depends on the cell's own height. `bottom: 2%` for this one instead expresses the
        // actually-intended "pinned near the bottom-right corner" relationship directly,
        // independent of cell height — the same reasoning that motivated `right` over `left`
        // for the horizontal axis.
        [corner === 'top' ? 'top' : 'bottom']: '2%',
        right: '2%',
        padding: 12,
        fontSize: 20,
        color: 'lightskyblue',
        // Mirrors legacy's own real `.quick-thumbnail:hover>.page-number` rule (`lrr.css`) —
        // `z-index: -1` at rest (behind the thumbnail `<img>`, effectively invisible), `300` +
        // a black backdrop once actually hovered, driven here by this component's own React
        // state rather than that shared CSS selector (see this component's own docs for why).
        zIndex: hovered ? 300 : -1,
        backgroundColor: hovered ? '#000000' : undefined,
      }}
    />
  )
}

const QUICK_ADD_TOC_CHAPTER_COUNT = 15

/** Right-click menu on the "add chapter" icon (`PageGridActionIcon`'s `fa-book-medical`) — a
 * purely additive shortcut, no legacy equivalent, for the handful of chapter titles common enough
 * in real doujin/manga scans to not need typing out via the plain left-click `promptDialog` flow
 * every time. Every option submits immediately on pick (no separate confirm step) — the point is
 * speed for a title that's already fully decided the moment it's clicked/selected, not a form. */
function QuickAddTocPopover({
  x,
  y,
  onPick,
  onClose,
}: {
  x: number
  y: number
  onPick: (title: string) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const presets: { icon: string; title: string }[] = [
    { icon: 'fa-file-image', title: t('Cover') ?? 'Cover' },
    { icon: 'fa-file-image', title: t('Back Cover') ?? 'Back Cover' },
    { icon: 'fa-list', title: t('Table of Contents') ?? 'Table of Contents' },
    { icon: 'fa-palette', title: t('Color Pages') ?? 'Color Pages' },
  ]
  return (
    <>
      {/* `stopPropagation` — this backdrop's own click-to-close would otherwise bubble up to
          `#overlay-shade` (the outer Archive Overview modal's own click-to-close backdrop,
          covering the same full viewport) and close *that* too, since neither backdrop is a DOM
          ancestor/descendant of the other that a plain click could be scoped to (confirmed live:
          clicking outside this popover closed the whole overview modal along with it). */}
      <div
        style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          e.stopPropagation()
        }}
      />
      <PopupMenu style={{ position: 'fixed', top: y, left: x, zIndex: Z_OVERLAY_CONTENT }}>
        {presets.map(({ icon, title }) => (
          <PopupMenuItem
            key={title}
            onClick={(e) => {
              // Without this, the click bubbles up to the page-grid cell's own
              // `onClick={() => onSelectPage(page)}` (this popover's trigger icon sits inside
              // that cell) — the chapter got added correctly, but the reader then also navigated
              // to that page, an unwanted extra side effect this popover was never meant to have.
              e.stopPropagation()
              onPick(title)
              onClose()
            }}
          >
            <i className={`fa ${icon}`} style={{ width: 18 }}></i> {title}
          </PopupMenuItem>
        ))}
        <PopupMenuItem style={{ cursor: 'default' }}>
          <i className="fa fa-book-medical" style={{ width: 18 }}></i>
          <select
            className="favtag-btn"
            defaultValue=""
            style={{ marginLeft: 4 }}
            onClick={(e) => e.stopPropagation()}
            onChange={(e) => {
              if (!e.target.value) return
              onPick(t('Chapter {{n}}', { n: e.target.value }) ?? `Chapter ${e.target.value}`)
              onClose()
            }}
          >
            <option value="" disabled>
              {t('Chapter…')}
            </option>
            {Array.from({ length: QUICK_ADD_TOC_CHAPTER_COUNT }, (_, i) => i + 1).map((n) => (
              <option key={n} value={n}>
                {t('Chapter {{n}}', { n })}
              </option>
            ))}
          </select>
        </PopupMenuItem>
      </PopupMenu>
    </>
  )
}

// Mirrors legacy's `#archivePagesOverlay` (`updateArchiveOverlay`/`generateThumbnails` in
// `~/LANraragi/public/js/reader.js`) — thumbnail (left) + Admin Options/Categories/Rating (right)
// side by side via `.reader-thumbnail`'s `display:inline-block` (verified against
// `~/LANraragi/public/css/lrr.css`), the full per-namespace tags table below it, then a thumbnail
// grid scoped to the current chapter (or the whole archive if there's no TOC).
export default function ArchiveOverviewOverlay({
  archive,
  categories,
  loggedIn,
  currentPage,
  onClose,
  onSelectPage,
}: {
  archive: ArchiveMetadata
  categories: CategoryMetadata[] | undefined
  loggedIn: boolean
  currentPage: number
  onClose: () => void
  onSelectPage: (page: number) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const staticCategories = (categories ?? []).filter((c) => !c.search)
  const archiveCategories = staticCategories.filter((c) => c.archives.includes(archive.arcid))

  // Legacy's `#filter-stamped` (`reader.js`'s `checkStampedPages`/`filterStampedOverlay`) — marks
  // each thumbnail `data-stamped=true` if `GET /archives/{id}/stamps` includes its page number,
  // then a toggle hides every non-stamped thumbnail so the grid becomes a stamped-pages-only view.
  const stampedPages = useStampedPages(archive.arcid)
  const stampedPageSet = new Set(stampedPages.data?.result ?? [])
  const [filterStamped, setFilterStamped] = useState(false)

  const chapters = archive.toc.length > 0 ? archive.toc : null

  // Legacy's `getCurrentChapter` (`reader.js`) — the last ToC entry whose `startPage` is `<=` the
  // reader's current page; only leaf chapters (this port has no sub-chapter nesting) get
  // edit/delete icons (legacy: `currentChapter.chapters === null`).
  const currentChapter = chapters
    ? [...chapters].filter((c) => c.page <= currentPage).sort((a, b) => b.page - a.page)[0]
    : undefined

  const setThumbnail = useSetArchiveThumbnail(archive.arcid)
  const addTocEntry = useAddTocEntry(archive.arcid)
  const removeTocEntry = useRemoveTocEntry(archive.arcid)

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
  // thumbnail's own `onSelectPage` navigation.
  function handleSetThumbnail(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
    setThumbnail.mutate(page, {
      onSuccess: () => {
        setThumbnailVersion((v) => v + 1)
        toast({ text: t('Successfully set page {{n}} as the thumbnail!', { n: page }) ?? undefined })
      },
      onError: () => toast({ text: t('Error updating thumbnail') ?? undefined, icon: 'error' }),
    })
  }

  // Legacy's `.add-toc` click handler + `addTocSection` (`reader.js`) — prompts for a chapter
  // title, then PUTs the new ToC entry. Empty/cancelled input adds nothing (matches legacy's own
  // `result.value.trim() !== ""` guard).
  async function handleAddToc(e: MouseEvent, page: number) {
    e.preventDefault()
    e.stopPropagation()
    const title = await promptDialog(t('Enter a title for this chapter/section:') ?? '')
    if (title && title.trim() !== '') {
      addTocEntry.mutate(
        { page, title: title.trim() },
        { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
      )
    }
  }

  // Right-click on the same "add chapter" icon (see `handleAddToc` above for its plain left-click
  // prompt-based flow) — a purely additive shortcut (no legacy equivalent) for the handful of
  // chapter titles that come up often enough in real doujin/manga scans to not need typing out
  // every time: 封面/封底/目录/彩页, plus 第N章 (1–15) via a `<select>`. Submits immediately on
  // pick — no separate confirm step, matching this popover's own single-click-and-done feel
  // rather than a form the user has to explicitly submit.
  function handleQuickAddToc(page: number, title: string) {
    addTocEntry.mutate(
      { page, title },
      { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
    )
  }

  // Legacy's `.edit-toc` click handler (`reader.js`: `addTocSection(currentChapter.startPage,
  // currentChapter.name)`) — re-prompts with the existing name pre-filled as a placeholder, then
  // re-adds the entry at the same page (the host's `add_toc_entry` replaces same-page entries
  // rather than duplicating them, matching legacy's own upsert-by-page semantics).
  async function handleEditToc() {
    if (!currentChapter) return
    const title = await promptDialog(t('Enter a title for this chapter/section:') ?? '', currentChapter.name)
    if (title && title.trim() !== '') {
      addTocEntry.mutate(
        { page: currentChapter.page, title: title.trim() },
        { onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }) },
      )
    }
  }

  // Legacy's `.remove-toc` click handler + `removeTocSection` (`reader.js`).
  async function handleRemoveToc() {
    if (!currentChapter) return
    if (!(await confirmDialog(t('Are you sure you want to delete this chapter/section?') ?? ''))) return
    removeTocEntry.mutate(currentChapter.page, {
      onError: () => toast({ text: t('Error adding/removing chapter:') ?? undefined, icon: 'error' }),
    })
  }

  async function addToCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'PUT' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function removeFromCategory(categoryId: string) {
    await fetch(`/api/categories/${categoryId}/${archive.arcid}`, { method: 'DELETE' })
    await queryClient.invalidateQueries({ queryKey: ['categories'] })
  }

  async function deleteArchive() {
    if (
      !(await confirmDialog(
        t('This will delete both metadata and matching files from your system! Please use with caution.') ?? '',
      ))
    ) {
      return
    }
    await fetch(`/api/archives/${archive.arcid}`, { method: 'DELETE' })
    navigate(routes.library())
  }

  const pageCount = archive.pagecount

  return (
    <>
      {/* `#overlay-shade` starts `display:none` in `lrr.css` — legacy's own JS explicitly shows it
          (`fadeTo`) when opening an overlay rather than relying on presence in the DOM, so this
          needs the same explicit override or clicking it (or even seeing it) does nothing. */}
      {/* Legacy shows this via `.fadeTo(150, 0.6, ...)` — animates to 60% opacity, not fully
          opaque black, so content behind the shade stays faintly visible. */}
      <div id="overlay-shade" style={{ display: 'block', opacity: 0.6 }} onClick={onClose} />
      <div id="archivePagesOverlay" className="id1 base-overlay page-overlay">
        <h2 className="ih" style={{ textAlign: 'center' }}>
          {t('Archive Overview')}
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
                src={`/api/archives/${archive.arcid}/thumbnail${thumbnailVersion > 0 ? `?v=${thumbnailVersion}` : ''}`}
                style={{ maxWidth: '100%' }}
              />
            </div>

            {loggedIn && (
              <div style={{ display: 'inline-block', verticalAlign: 'middle' }}>
                <h2>{t('Admin Options')}</h2>

                <input
                  className="stdbtn"
                  type="button"
                  value={t('Edit Archive Metadata') ?? undefined}
                  onClick={() => navigate(routes.edit(archive.arcid))}
                />
                <input
                  className="stdbtn"
                  type="button"
                  value={t('Delete Archive') ?? undefined}
                  onClick={() => void deleteArchive()}
                />
                <br />

                <h2>{t('Categories')}</h2>
                <div style={{ display: 'inline-block' }}>
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
                <span>{t('Add to : ')}</span>
                <select
                  id="category"
                  className="favtag-btn"
                  style={{ width: 200 }}
                  value=""
                  onChange={(e) => {
                    if (e.target.value) void addToCategory(e.target.value)
                  }}
                >
                  <option value="">{t(' -- No Category -- ')}</option>
                  {staticCategories.map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
                </select>

                <h2>{t('Rating')}</h2>
                <RatingWidget archiveId={archive.arcid} tags={archive.tags} />
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
                className={`fas fa-stamp${filterStamped ? ' toggled' : ''}`}
                id="filter-stamped"
                href="#"
                style={{ padding: 8, fontSize: 14 }}
                title={t('Filter stamped pages') ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setFilterStamped((v) => !v)
                }}
              />
            )}
          </div>
          <h2 className="ih">{chapters ? t('Chapters') : t('Pages')}</h2>
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
                    {c.name}
                  </option>
                ))}
              </select>
            )}
            {loggedIn && chapters && currentChapter && (
              <>
                <a
                  className="fas fa-pencil-alt edit-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14 }}
                  title={t('Edit Chapter name') ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    void handleEditToc()
                  }}
                />
                <a
                  className="fas fa-trash-alt remove-toc"
                  href="#"
                  style={{ padding: 8, fontSize: 14 }}
                  title={t('Delete Chapter') ?? undefined}
                  onClick={(e) => {
                    e.preventDefault()
                    void handleRemoveToc()
                  }}
                />
              </>
            )}
          </div>
        </div>

        <div id="pages-section" style={{ textAlign: 'center' }}>
          {Array.from({ length: pageCount }, (_, i) => i + 1).map((page) => {
            const isStamped = stampedPageSet.has(String(page))
            if (filterStamped && !isStamped) return null
            return (
              <PageGridCell
                key={page}
                page={page}
                isStamped={isStamped}
                loggedIn={loggedIn}
                thumbnailSrc={`/api/archives/${archive.arcid}/thumbnail?page=${page}`}
                onSelectPage={onSelectPage}
                onSetThumbnail={handleSetThumbnail}
                onAddToc={handleAddToc}
                onQuickAddToc={handleQuickAddToc}
              />
            )
          })}
        </div>
      </div>
    </>
  )
}

/** One cell in the page-grid — split out from the inline map body so the hover state that both
 * `PageGridActionIcon`s need (see that component's own docs on why it can't track its own hover)
 * has somewhere to live: the parent `.quick-thumbnail` cell itself, exactly like legacy's own
 * `.quick-thumbnail:hover>.page-number` CSS rule keys off the same element. */
function PageGridCell({
  page,
  isStamped,
  loggedIn,
  thumbnailSrc,
  onSelectPage,
  onSetThumbnail,
  onAddToc,
  onQuickAddToc,
}: {
  page: number
  isStamped: boolean
  loggedIn: boolean
  thumbnailSrc: string
  onSelectPage: (page: number) => void
  onSetThumbnail: (e: MouseEvent, page: number) => void
  onAddToc: (e: MouseEvent, page: number) => void
  onQuickAddToc: (page: number, title: string) => void
}) {
  const { t } = useTranslation()
  const [hovered, setHovered] = useState(false)
  const [quickAddAt, setQuickAddAt] = useState<{ x: number; y: number } | null>(null)
  return (
    <div
      className="id1"
      style={{ display: 'inline-block', cursor: 'pointer' }}
      onClick={() => onSelectPage(page)}
    >
      <div
        className="id3 quick-thumbnail"
        data-stamped={isStamped || undefined}
        style={{ position: 'relative' }}
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <PageNumberLabel>{t('Page {{n}}', { n: page })}</PageNumberLabel>
        <OverviewThumbnail src={thumbnailSrc} alt={t('Page {{n}}', { n: page }) ?? undefined} />
        {loggedIn && (
          <>
            <PageGridActionIcon
              icon="fa-file-image"
              corner="top"
              title={t('Set this Page as Thumbnail') ?? undefined}
              hovered={hovered}
              onClick={(e) => onSetThumbnail(e, page)}
            />
            {/* `wrapperStyle={{ position: 'static' }}` — `Tooltip`'s own default wrapper is
                `position: relative`, which silently became this icon's *new* positioning
                containing block once it started wrapping it (the icon itself is `position:
                absolute; bottom: 2%` — a real, live-confirmed regression: without this override,
                that 2% resolved against the wrapper's own ~44px shrink-to-fit height instead of
                `.quick-thumbnail`'s real ~280px, landing the icon far too high). `Tooltip`'s own
                positioning math (`getBoundingClientRect()` on the wrapper) doesn't depend on
                `position: relative` at all, so this is a safe override. */}
            <Tooltip
              label={t('Add Chapter at this Page') + ' ' + t('(right-click for quick presets)')}
              wrapperStyle={{ position: 'static' }}
            >
              <PageGridActionIcon
                icon="fa-book-medical"
                corner="bottom"
                hovered={hovered}
                onClick={(e) => onAddToc(e, page)}
                onContextMenu={(e) => {
                  e.preventDefault()
                  e.stopPropagation()
                  setQuickAddAt({ x: e.clientX, y: e.clientY })
                }}
              />
            </Tooltip>
          </>
        )}
      </div>
      {quickAddAt && (
        <QuickAddTocPopover
          x={quickAddAt.x}
          y={quickAddAt.y}
          onPick={(title) => onQuickAddToc(page, title)}
          onClose={() => setQuickAddAt(null)}
        />
      )}
    </div>
  )
}
