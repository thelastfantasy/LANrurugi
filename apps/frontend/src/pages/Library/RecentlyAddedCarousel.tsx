import "swiper/css"
import "swiper/css/navigation"

import type { MouseEvent } from "react"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"
import { Mousewheel, Navigation } from "swiper/modules"
import { Swiper, SwiperSlide } from "swiper/react"

import type { ArchiveMetadata } from "@/api/types"
import { PopupMenu, PopupMenuItem } from "@/components/Display"
import { SortableList } from "@/components/Display"
import { CAROUSEL_ICON, NEW_ONLY, UNTAGGED_ONLY } from "@/lib/constants"
import { CAROUSEL_OPEN_KEY, CAROUSEL_TYPE_KEY } from "@/lib/storageKeys"
import { Z_OVERLAY_CONTENT } from "@/theme"

import { CarouselCard } from "./CarouselCard"
import { SelectedArchiveSlideContent } from "./SelectedArchiveSlideContent"
import { type CarouselMode } from "./types"


export function RecentlyAddedCarousel({
  filter,
  category,
  hideCompleted,
  groupbyTanks,
  cropThumbs,
  onContextMenu,
  onOpen,
  multiSelect,
  selectedIds,
  onToggleSelected,
  onReorderSelection,
  onSelectPage,
  onClearSelection,
  onRunBatch,
  onMerge,
  canMerge,
  onSearchTag,
  refreshKey,
}: {
  filter: string
  category: string
  hideCompleted: boolean
  groupbyTanks: boolean
  cropThumbs: boolean
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata, source: "carousel") => void
  onOpen: (id: string) => void
  multiSelect: boolean
  selectedIds: string[]
  onToggleSelected: (id: string) => void
  /** Drag-to-reorder in the selection list below — additive (legacy has no such capability at
   * all, its own selection list is a plain unordered `Set`). The new order becomes the merged
   * Tankoubon's own volume order (`archives`, itself order-significant) when `onMerge` folds the
   * selection into one, rather than whatever arbitrary order clicking each archive happened in. */
  onReorderSelection: (newOrder: string[]) => void
  onSelectPage: () => void
  onClearSelection: () => void
  onRunBatch: () => void
  onMerge: () => void
  canMerge: boolean
  onSearchTag: (namespacedTag: string) => void
  /** Bumped by the parent whenever something outside this component's own control (the
   * "Mark as Read"/"Mark as Unread" context-menu action, currently the only such case) changes
   * archive progress data this carousel's own fetch effect has no other way to learn about — this
   * carousel doesn't use TanStack Query (a plain `useEffect`+`fetch`, unlike the main grid's own
   * `useSearch`), so a parent-side `invalidateQueries` call has no effect on it at all. */
  refreshKey: number
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(() => localStorage.getItem(CAROUSEL_OPEN_KEY) === "1")
  const [mode, setMode] = useState<CarouselMode>(
    () => (localStorage.getItem(CAROUSEL_TYPE_KEY) as CarouselMode | null) ?? "ondeck",
  )
  const [menuOpen, setMenuOpen] = useState(false)
  const [items, setItems] = useState<ArchiveMetadata[]>([])
  const [loading, setLoading] = useState(false)
  const [nonce, setNonce] = useState(0)

  useEffect(() => {
    localStorage.setItem(CAROUSEL_OPEN_KEY, open ? "1" : "0")
  }, [open])

  // The mode-switch "..." dropdown is its own local overlay (not the page-level
  // `contextMenu`/`categoryOverflowOpen` state Escape already clears in the parent), so it needs
  // its own listener to close on Escape too.
  useEffect(() => {
    if (!menuOpen) return
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") setMenuOpen(false)
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [menuOpen])
  useEffect(() => {
    localStorage.setItem(CAROUSEL_TYPE_KEY, mode)
  }, [mode])

  // Legacy's own `enterSelectionCarouselMode` (`index.js`) force-expands the carousel — entering
  // MSM with it collapsed would otherwise hide the very selection list the mode exists to show.
  // A derived value (not an effect calling `setOpen`) since this is plain synchronous rendering
  // logic, not a sync-with-an-external-system concern — `open` itself still holds only the user's
  // own manually-toggled preference (that's what's persisted to `localStorage` above), unaffected
  // by MSM's temporary forced-expand.
  const isOpen = open || multiSelect

  useEffect(() => {
    // No mode-based fetch while in selection mode — legacy doesn't refresh carousel data during
    // MSM either, the carousel is repurposed to display the selection itself instead.
    if (!isOpen || multiSelect) return
    const params = new URLSearchParams()
    if (filter) params.set("filter", filter)
    const isBuiltinSelector = category === NEW_ONLY || category === UNTAGGED_ONLY
    if (category && !isBuiltinSelector) params.set("category", category)
    if (!groupbyTanks) params.set("groupby_tanks", "false")
    if (hideCompleted) params.set("hidecompleted", "true")
    if (category === NEW_ONLY) params.set("newonly", "true")
    if (category === UNTAGGED_ONLY) params.set("untaggedonly", "true")

    let endpoint: string
    switch (mode) {
      case "random":
        params.set("count", "15")
        endpoint = `/api/search/random?${params.toString()}`
        break
      case "inbox":
        params.set("newonly", "true")
        params.set("sortby", "date_added")
        params.set("order", "desc")
        params.set("start", "-1")
        endpoint = `/api/search?${params.toString()}`
        break
      case "untagged":
        params.set("untaggedonly", "true")
        params.set("sortby", "date_added")
        params.set("order", "desc")
        params.set("start", "-1")
        endpoint = `/api/search?${params.toString()}`
        break
      default:
        params.set("sortby", "lastread")
        params.set("hidecompleted", "true")
        endpoint = `/api/search?${params.toString()}`
        break
    }

    let cancelled = false
    // `setLoading(true)` is deferred a tick rather than called synchronously in the effect body —
    // this is a real network request kicking off (an external-system interaction, not a plain
    // state sync), and react-hooks' `set-state-in-effect` rule flags direct synchronous setState
    // calls in an effect body as a cascading-render risk regardless.
    queueMicrotask(() => {
      if (!cancelled) setLoading(true)
    })
    fetch(endpoint)
      .then((r) => r.json())
      .then((data) => {
        if (cancelled) return
        setItems(data.data ?? [])
      })
      .finally(() => {
        if (!cancelled) setLoading(false)
      })
    return () => {
      cancelled = true
    }

  }, [isOpen, mode, filter, category, hideCompleted, groupbyTanks, nonce, multiSelect, refreshKey])

  const modeLabel: Record<CarouselMode, string> = {
    ondeck: t("On Deck"),
    random: t("Random"),
    inbox: t("New Archives"),
    untagged: t("Untagged Archives"),
  }

  return (
    <ul className="collapsible index-carousel with-right-caret">
      {/* Real legacy class list is exactly `collapsible index-carousel with-right-caret` with no
          inline style — `index-carousel`'s CSS (`lrr.css`) supplies the panel's own margins/inset,
          and `.option-flyout>.collapsible-title` is a direct-child selector, so
          `.collapsible-title`/`.collapsible-right` must stay direct children of `.option-flyout`
          (no wrapper div) or that styling silently drops. */}
      <li
        className="option-flyout"
        style={{ display: "flex", flexWrap: "wrap", justifyContent: "space-between" }}
      >
        {/* Matches legacy's real two-sibling split — `caret-right`'s CSS `::after` glyph paints
            at the end of whichever element carries the class, so keeping the refresh/more-options
            buttons out of `.collapsible-title` entirely puts the caret right after "On Deck"
            instead of at the far right past both buttons. */}
        <div
          className={`collapsible-title caret-right${isOpen ? " active" : ""}`}
          onClick={() => setOpen((o) => !o)}
          style={{ display: "flex", alignItems: "center", flex: "1 1 0", overflow: "hidden" }}
        >
          {/* Legacy's `enterSelectionCarouselMode`/`exitSelectionCarouselMode` (`index.js`) swap
              this same header's icon/text in place rather than showing a second header — MSM is a
              *mode the carousel itself enters*, not a separate panel underneath it. */}
          <i className={multiSelect ? "fas fa-check-square" : `fa ${CAROUSEL_ICON[mode]}`} aria-hidden="true"></i>
          <div style={{ marginLeft: 8 }}>{multiSelect ? t("Selection") : modeLabel[mode]}</div>
        </div>
        {isOpen && multiSelect && (
          <div className="collapsible-right" onClick={(e) => e.stopPropagation()}>
            {/* Legacy's real 4-button MSM toolbar lives in this exact `.collapsible-right`
                slot, replacing the refresh/more-options icons. `updateSelectionCount` hides
                batch-ops/merge/clear at zero selected — only select-page stays visible. */}
            {selectedIds.length > 0 && (
              <span>{t("{{n}} selected", { n: selectedIds.length })}</span>
            )}
            {/* No `marginBottom` offset on these four, unlike the refresh/more-options icons
                below (`margin-bottom: 0px` vs. `-4px` in legacy's real computed style). */}
            {selectedIds.length > 0 && (
              <a
                href="#"
                className="fa fa-2x fa-hammer"
                style={{ marginLeft: 12 }}
                title={t("Run Batch Operations on selection") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onRunBatch()
                }}
              ></a>
            )}
            {canMerge && (
              <a
                href="#"
                className="fa fa-2x fa-compress-alt"
                style={{ marginLeft: 12 }}
                title={t("Merge Archives into Tankoubon") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onMerge()
                }}
              ></a>
            )}
            {selectedIds.length > 0 && (
              <a
                href="#"
                className="fa fa-2x fa-eject"
                style={{ marginLeft: 12 }}
                title={t("Clear selection") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  onClearSelection()
                }}
              ></a>
            )}
            <a
              href="#"
              className="fa fa-2x fa-check-double"
              style={{ marginLeft: 12 }}
              title={t("Select All in Page") ?? undefined}
              onClick={(e) => {
                e.preventDefault()
                onSelectPage()
              }}
            ></a>
          </div>
        )}
        {isOpen && !multiSelect && (
          <div className="collapsible-right" onClick={(e) => e.stopPropagation()}>
            <a
              href="#"
              className={`fa fa-2x fa-sync${loading ? " fa-spin" : ""}`}
              style={{ marginBottom: -4 }}
              title={t("Refresh") ?? undefined}
              onClick={(e) => {
                e.preventDefault()
                setNonce((n) => n + 1)
              }}
            ></a>
            <span style={{ position: "relative" }}>
              <a
                href="#"
                className="fa fa-2x fa-ellipsis-h"
                style={{ marginBottom: -4, marginLeft: 12 }}
                title={t("Carousel Mode") ?? undefined}
                onClick={(e) => {
                  e.preventDefault()
                  setMenuOpen((m) => !m)
                }}
              ></a>
              {menuOpen && (
                // Not portaled — positioned via `top: '100%'`/`right: 0` against this menu's own
                // trigger `<span style={{ position: 'relative' }}>`, which a default portal to
                // `document.body` would detach it from.
                <PopupMenu
                  portal={false}
                  style={{ position: "absolute", top: "100%", right: 0, zIndex: Z_OVERLAY_CONTENT }}
                >
                  {(["ondeck", "random", "inbox", "untagged"] as CarouselMode[]).map((m) => (
                    <PopupMenuItem
                      key={m}
                      style={{ fontWeight: m === mode ? "bold" : undefined }}
                      onClick={() => {
                        setMode(m)
                        setMenuOpen(false)
                      }}
                    >
                      <i className={`fa ${CAROUSEL_ICON[m]}`} aria-hidden="true"></i> {modeLabel[m]}
                    </PopupMenuItem>
                  ))}
                </PopupMenu>
              )}
            </span>
          </div>
        )}
        {isOpen && multiSelect && (
          // Legacy's carousel body is repurposed into the selection list itself during MSM,
          // reusing the same empty-state icon/copy the normal "no results" state has.
          //
          // `boxSizing: 'border-box'` matters here: `.option-flyout>.collapsible-body`'s real CSS
          // gives it `padding: 10px !important`, which under content-box sizing would add on top
          // of `width: 100%` instead of being included in it, overflowing the `<li>`'s right edge.
          <div className="collapsible-body" style={{ width: "100%", boxSizing: "border-box" }}>
            {selectedIds.length === 0 ? (
              /* The empty state reuses the grid card's own DOM skeleton (`div.id1` > `id2` +
                 `id3` + `id4`, exactly `ArchiveCard`'s structure) instead of a hardcoded height —
                 the card's height is pure CSS (`id2` 30px + `id3` 280px desktop / 196px under
                 `lrr.css`'s `max-width: 560px` breakpoint + `id4` 20px + `id1`'s `padding-top`),
                 so this panel matches the grid's cards on any device/theme with no measurement
                 and no state. The outer `padding: '8px 0'` wrapper is the *same* wrapper the
                 populated branch's `SortableList` row sits in — without it the empty state
                 rendered 16px shorter than the populated one (the row padding's height), a
                 live-confirmed panel-height jump on selecting the first archive. The hint text
                 sits centered inside the `id3` cover slot. */
              <div style={{ padding: "8px 0" }}>
                <div className="id1" style={{ width: "100%", boxSizing: "border-box" }}>
                  <div className="id2"></div>
                  <div
                    className="id3"
                    style={{ display: "flex", flexDirection: "column", alignItems: "center", justifyContent: "center" }}
                  >
                    <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                    <span style={{ marginTop: 12 }}>
                      {t("Click Archives to add them to the selection. Your selection carries over across searches.")}
                    </span>
                  </div>
                  <div className="id4"></div>
                </div>
              </div>
            ) : (
              // Deliberately not the `<Swiper>` the read-only carousel modes below use — `swiper/
              // react` requires `SwiperSlide` to be a direct JSX child of `Swiper` (see
              // `SelectedArchiveSlideContent`'s own docs for the real bug that comes from breaking
              // that), and combining Swiper's own slide-transform positioning with dnd-kit's
              // sortable transforms on the *same* elements would be fighting two libraries over the
              // same job. `SortableList`'s `horizontal` direction gives the same "row of cards,
              // scrolls instead of wrapping" look via a plain `overflow-x: auto` flex row.
              <div style={{ padding: "8px 0" }}>
                <SortableList
                  items={selectedIds}
                  getId={(id) => id}
                  direction="horizontal"
                  onReorder={onReorderSelection}
                  renderItem={(id, dragHandleProps) => (
                    <div
                      {...dragHandleProps.attributes}
                      {...dragHandleProps.listeners}
                      // `.carousel-slide` (`index.css`) — same responsive-width fix as the
                      // read-only carousel above, and the same bug: a plain inline `width: 228`
                      // can't respond to `lrr.css`'s own `max-width: 560px` breakpoint that
                      // `div.id1` (inside `SelectedArchiveSlideContent`) shrinks to 164px under.
                      className="carousel-slide"
                      style={{
                        marginRight: 8,
                        cursor: dragHandleProps.isDragging ? "grabbing" : "grab",
                      }}
                    >
                      <SelectedArchiveSlideContent
                        id={id}
                        cropThumbs={cropThumbs}
                        onContextMenu={(e, archive) => onContextMenu(e, archive, "carousel")}
                        onRemove={onToggleSelected}
                      />
                    </div>
                  )}
                />
              </div>
            )}
          </div>
        )}
        {isOpen && !multiSelect && (
          // Same `boxSizing: 'border-box'` reasoning as the MSM branch above.
          <div className="collapsible-body" style={{ width: "100%", boxSizing: "border-box" }}>
            {loading && items.length === 0 ? (
              <div style={{ height: 344, display: "flex", justifyContent: "center", alignItems: "center" }}>
                <i className="fa fa-stroopwafel fa-spin fa-4x" aria-hidden="true"></i>
              </div>
            ) : items.length === 0 ? (
              <div style={{ height: 344, display: "flex", justifyContent: "center", alignItems: "center", flexDirection: "column" }}>
                <i className="fa fa-glasses fa-4x" aria-hidden="true"></i>
                <span style={{ marginTop: 12 }}>{t("No results here.")}</span>
              </div>
            ) : (
              <Swiper
                modules={[Navigation, Mousewheel]}
                navigation={{ nextEl: ".carousel-next", prevEl: ".carousel-prev" }}
                mousewheel
                spaceBetween={8}
                slidesPerView="auto"
                style={{ padding: "8px 0" }}
              >
                {items.map((a) => (
                  // `.carousel-slide` (`index.css`) tracks `div.id1`'s own responsive width exactly
                  // (228px desktop / 164px under `lrr.css`'s `max-width: 560px` breakpoint) — a
                  // plain inline `style={{ width: 228 }}` used to sit here instead, which matched
                  // the desktop case but, being an inline style, always won over the CSS cascade
                  // (including that same media query) on narrower viewports too, wrapping a
                  // genuinely 164px-wide mobile card in a stale 228px slide and leaving a large
                  // empty gap around the thumbnail. Swiper's `slidesPerView="auto"` mode needs each
                  // slide to carry its own explicit width to size correctly — it doesn't measure a
                  // child's rendered content width on its own.
                  <SwiperSlide key={a.arcid} className="carousel-slide">
                    <CarouselCard
                      archive={a}
                      cropThumbs={cropThumbs}
                      onContextMenu={(e, archive) => onContextMenu(e, archive, "carousel")}
                      onOpen={onOpen}
                      onSearchTag={onSearchTag}
                    />
                  </SwiperSlide>
                ))}
                <a href="#" className="fa fa-3x fa-chevron-left carousel-prev" style={{ position: "absolute", left: 0, top: 136, cursor: "pointer", zIndex: 20 }}></a>
                <a href="#" className="fa fa-3x fa-chevron-right carousel-next" style={{ position: "absolute", right: 0, top: 136, cursor: "pointer", zIndex: 20 }}></a>
              </Swiper>
            )}
          </div>
        )}
      </li>
    </ul>
  )
}
