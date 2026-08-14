import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useAddStamp, useDeleteStamp, useStampsForPage, useUpdateStamp } from "@/api/hooks"
import type { StampJson } from "@/api/types"
import { PopupMenu, PopupMenuItem } from "@/components/Display"
import { Tooltip } from "@/components/Display"
import {
  anchorPercent,
  formatStampRect,
  lastPickedRectStyle,
  parseStampRect,
  renderStampIcon,
  type StampAnchor,
  stampEditorDialog,
  type StampRect,
} from "@/dialog"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

// Mirrors legacy's stamp/marker feature (`~/LANraragi/public/js/reader.js`'s `addStamp`/
// `renderMarkers`/`loadStamps` + `.marker` in `lrr.css`): click-to-place a pin at a %-based
// position with a text label, drag to reposition, right-click to rename/delete. `imageRef` is the
// currently-visible page `<img>` a click's percentage position is measured against; `page` is
// that image's 1-indexed page number (the "left" slot in a double-page spread).
//
// A stamp placed by *dragging* (not a plain click) instead carries a selection rectangle
// alongside its plain point — `stamp.rect`, `"x,y,width,height,anchor,color"` (see `StampRect`'s
// own docs in `dialog.tsx`). The rect's own outline is hidden by default and only reveals on
// hover; clicking the icon selects it, which additionally shows 8 resize handles and allows
// dragging the rect itself to move both it and the icon's `position` together.

/** Minimum drag distance (in rendered image pixels) before a placement-mode drag counts as
 * "dragged a rectangle" rather than "a plain click that happened to jitter a pixel or two" — a
 * literal 0px threshold would make it effectively impossible to ever place a plain point stamp
 * with a mouse, since real pointer input essentially never reports two identical coordinates
 * across a mousedown/mouseup pair. */
const RECT_DRAG_THRESHOLD_PX = 4

/** Rectangle resize handles, matching `StampAnchor` — dragging one moves that corner/edge only,
 * keeping the opposite corner/edge fixed (standard resize-handle semantics). */
const RESIZE_HANDLES: StampAnchor[] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"]

interface ContextMenuState {
  stampId: string
  x: number
  y: number
}

/** A drag in progress: `stampId` being moved, and its live position while dragging (not yet
 * persisted — only sent to the server on pointer-up, matching legacy's own drop-to-commit UX
 * rather than firing a PUT on every mousemove). */
interface DragState {
  stampId: string
  x: number
  y: number
}

export function MarkerLayer({
  archiveId,
  page,
  imageRef,
  visible,
  placementMode,
  onPlaced,
}: {
  archiveId: string
  page: number
  imageRef: React.RefObject<HTMLImageElement | null>
  visible: boolean
  /** True while the user has pressed `S` and is about to click a spot to drop a pin — legacy's
   * `markerMode` (`addStamp()` in reader.js arms it, the next click on the image consumes it). */
  placementMode: boolean
  onPlaced: () => void
}) {
  const { t } = useTranslation()
  const stamps = useStampsForPage(archiveId, page)
  const addStamp = useAddStamp(archiveId)
  const updateStamp = useUpdateStamp()
  const deleteStamp = useDeleteStamp()
  // `.marker`'s CSS positioning (`left`/`top` as a plain `%`) resolves against its *own* nearest
  // positioned ancestor via the normal CSS cascade — which, since this component renders as a
  // sibling of `#imgLink` (`Reader.tsx`), not a child of it, is `#i1.sni` (the whole reader page's
  // outer container, including the title bar/paginator/etc.), not the image. This wrapper gives
  // every `%`-based marker position a positioning context that's pixel-for-pixel the image's own
  // real box instead, kept in sync via `ResizeObserver` (window resizes, fit-mode changes,
  // container-width changes — anything that can resize or reposition the image without
  // necessarily causing *this* component to re-render on its own).
  //
  // Critically, the offset below is computed relative to `wrapperRef.current.offsetParent` — the
  // wrapper *itself*'s real CSS containing block — not `img.offsetParent`. An earlier version used
  // the image's own `offsetParent` (`#imgLink`, which *is* `position: relative` in `Reader.tsx`),
  // reasoning "the wrapper needs to be positioned relative to whatever the image is relative to."
  // But `MarkerLayer` renders as a *sibling* of `#imgLink` (inside `#display`), not a child of it —
  // CSS positioning can only ever resolve against a genuine ancestor, so the wrapper's `left`/`top`
  // in fact resolved against `#i1.sni` (the next positioned ancestor *up* from there) regardless of
  // what `img.offsetParent` reported in JS. The two philosophies "what is the image positioned
  // relative to" and "what is this wrapper positioned relative to" only coincide if the wrapper is
  // rendered as the image's own actual sibling *within its positioned parent* — which it isn't
  // here. Verified live: `img.offsetParent`-based math reported the wrapper as `1px, 1px` from
  // `#imgLink` (correct, if that were the real containing block), yet the wrapper's own actual
  // `getBoundingClientRect()` was offset by ~94px vertically / ~10px horizontally from the
  // image — exactly the gap between `#imgLink`'s and `#i1.sni`'s own positions on the page, i.e.
  // every rect/marker was being measured against one ancestor and rendered against a different one.
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [imgBounds, setImgBounds] = useState<{ left: number; top: number; width: number; height: number } | null>(null)
  useEffect(() => {
    const img = imageRef.current
    if (!img) return
    // Re-measured (not just re-observed) fresh on every `page` change — `MarkerLayer` itself
    // isn't remounted on page navigation (no `key={page}` on it in `Reader.tsx`; only the `page`
    // prop changes), so `imageRef` — a ref object, whose identity is stable for the component's
    // entire lifetime — never changed either. With only `[imageRef]` as a dependency, this effect
    // would only ever run once on mount: every later page navigation re-points `imageRef.current`
    // at a same-ID `<img>` element with a genuinely different size (a different page image, often
    // a different aspect ratio), but `ResizeObserver` only fires on *that DOM node's own* size
    // changing — not on layout-affecting changes elsewhere that shift *where* it ends up — so
    // `imgBounds` could stay stale indefinitely after the very first page.
    function updateBounds() {
      const wrapper = wrapperRef.current
      if (!img || !wrapper) return
      const imgRect = img.getBoundingClientRect()
      const containingBlock = wrapper.offsetParent as HTMLElement | null
      const parentRect = (containingBlock ?? document.documentElement).getBoundingClientRect()
      setImgBounds({
        left: imgRect.left - parentRect.left,
        top: imgRect.top - parentRect.top,
        width: imgRect.width,
        height: imgRect.height,
      })
    }
    updateBounds()
    // A `requestAnimationFrame` re-measurement, one frame after the synchronous call above —
    // catches a reflow that hasn't fully settled yet at the moment this effect first runs (this
    // wrapper's own `position: absolute` isn't applied until React actually commits it to the DOM,
    // one render behind this effect scheduling; a same-page reload/remount can also leave other
    // reader chrome above the image still settling into its own final layout for a frame).
    const rafId = requestAnimationFrame(updateBounds)
    // The image's own `load` event catches the case `updateBounds()`'s synchronous call above
    // can't: this effect can run (on a page-prop change) before the browser has finished loading
    // the new `src` and settled the image's final rendered size, especially crossing between very
    // different aspect ratios where the browser's own reflow takes an extra frame.
    img.addEventListener("load", updateBounds)
    const resizeObserver = new ResizeObserver(updateBounds)
    resizeObserver.observe(img)
    window.addEventListener("resize", updateBounds)
    return () => {
      cancelAnimationFrame(rafId)
      img.removeEventListener("load", updateBounds)
      resizeObserver.disconnect()
      window.removeEventListener("resize", updateBounds)
    }
  }, [imageRef, page])

  const [menu, setMenu] = useState<ContextMenuState | null>(null)
  // The dragged-to position, kept around *after* the drag ends too — not reset to `null` on
  // mouseup. Clearing it there used to mean the marker's rendered position immediately fell back
  // to `stored` (the last position `stamps.data` — server-cached — actually has), which is still
  // the *pre-drag* value until `updateStamp`'s own PUT-then-invalidate-then-refetch round trip
  // lands: a real, visible double-jump (snaps back to where the drag started, then forward again
  // to where it was dropped, once the refetch finally catches up). This value is already exactly
  // correct the instant the drag ends, so there's no reason to ever stop trusting it in favor of
  // a server round trip that's strictly slower to reflect the same answer.
  const [drag, setDrag] = useState<DragState | null>(null)
  // Separate from `drag` on purpose — this is *only* "is a drag on this stamp in progress right
  // now" (cursor styling), independent of whatever position is currently being rendered.
  const [activeDragStampId, setActiveDragStampId] = useState<string | null>(null)
  const draggedRef = useRef(false)

  // Which existing rect stamp the cursor is currently over (reveals its outline) — `null` when
  // not hovering any, or while the placement-mode backdrop/a drag is intercepting pointer events
  // anyway. Independent of `selectedStampId` below: hovering a stamp that isn't the selected one
  // still shows its outline, just without resize handles.
  const [hoveredStampId, setHoveredStampId] = useState<string | null>(null)
  // The rect stamp currently "opened" for adjustment (clicked once) — shows resize handles and
  // allows dragging the rect itself, in addition to the plain hover-reveal outline every rect
  // stamp already gets. Cleared by clicking anywhere else (the backdrop-less equivalent of a
  // typical "click outside to deselect" pattern, implemented via a window-level listener below).
  const [selectedStampId, setSelectedStampId] = useState<string | null>(null)
  // A rect stamp's own live edit — resizing (`handle` set) or moving the whole rect (`handle`
  // `null`) — kept around after pointer-up the same way `drag` is above, for the same reason (no
  // double-jump waiting on the PUT-then-refetch round trip). Note this value is NEVER reset to
  // `null` once a stamp has been edited even once — matching `drag`'s own docs, it just keeps
  // being the most up-to-date known position/geometry for that stamp indefinitely. Code that
  // wants "is an edit *currently in progress* right now" (as opposed to "has this stamp *ever*
  // been edited") must use `activeRectEditStampId` below instead, not infer it from this ever
  // being non-null for a given `stampId`.
  const [rectEdit, setRectEdit] = useState<{ stampId: string; rect: StampRect; handle: StampAnchor | null } | null>(
    null,
  )
  const rectEditedRef = useRef(false)
  // Debounces the server commit for arrow-key rect nudging (see the keyboard-shortcuts effect
  // below) — each keydown updates `rectEdit` immediately for responsive visual feedback, but
  // holding an arrow key down fires many keydown events in quick succession (OS key-repeat), and
  // sending a PUT for every single one of those would both hammer the API and race itself (each
  // response's own refetch could land after a *later* keystroke's own local state, snapping the
  // rect visually backward for a frame). Committing only once, a short idle period after the last
  // keydown, matches the same "local-first, commit-on-settle" shape as every pointer-drag handler
  // in this file already uses (they commit on `mouseup`, i.e. once the gesture itself is done).
  const arrowNudgeCommitTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  // Separate from `rectEdit` on purpose, mirroring `activeDragStampId` above for the identical
  // reason: *only* "is a rect edit on this stamp in progress right now" (used to suppress the
  // hover-leave outline-hide while mid-drag), independent of whatever geometry is currently being
  // rendered. Using `rectEdit?.stampId === stampId` for this instead was a real bug — `rectEdit`
  // itself is deliberately never cleared after the *first* edit, so that comparison stayed `true`
  // forever afterward, permanently blocking `onMouseLeave` from ever hiding the outline again once
  // a stamp had been dragged/resized even once. Verified live: after one resize-handle drag, the
  // outline stopped disappearing on mouse-leave for the rest of the session, for that stamp only.
  const [activeRectEditStampId, setActiveRectEditStampId] = useState<string | null>(null)

  // The live, still-being-dragged preview of a Ctrl+drag copy — deliberately *not* stored in
  // `rectEdit` (which represents the *original* stamp's own position/geometry) — the original is
  // never mutated by a copy-drag at all, so its own rendered outline must stay exactly where it
  // already was for the whole gesture, while a second, separate preview rect follows the cursor
  // to show where the about-to-be-created copy will actually land. Reusing `rectEdit` for both
  // would mean the *original's own* rendered rect moves with the cursor during a copy-drag — a
  // real bug reported live ("我看到的是复制源的选框随着复制被带走了") once the "does the outline
  // disappear" question turned out to actually be "the single rect visually follows the drag,
  // leaving nothing behind at the original spot" — the outline was never invisible, it just
  // wasn't a *second*, independent one.
  const [copyDragPreview, setCopyDragPreview] = useState<{ stampId: string; rect: StampRect } | null>(null)

  // The in-progress rectangle while dragging in placement mode — page-relative percent
  // coordinates for both the mousedown-anchored corner (`startX/startY`) and the current cursor
  // corner (`curX/curY`), from which the actual `x,y,width,height` (always non-negative) is
  // derived at render/submit time regardless of which direction the user dragged.
  const [placementDrag, setPlacementDrag] = useState<{ startX: number; startY: number; curX: number; curY: number } | null>(
    null,
  )

  function percentFromEvent(clientX: number, clientY: number): { x: number; y: number } | null {
    const img = imageRef.current
    if (!img) return null
    const rect = img.getBoundingClientRect()
    return {
      x: clampPercent(((clientX - rect.left) / rect.width) * 100),
      y: clampPercent(((clientY - rect.top) / rect.height) * 100),
    }
  }

  /** The highest `rect.layer` currently in use among this page's own rect stamps (`0` if there
   * are none yet) — used to put a freshly created rectangle on top of whatever's already there by
   * default, and by the bring-to-front keyboard shortcut below, rather than either of them ever
   * needing a fixed/arbitrary layer range of their own. */
  function maxLayerOnPage(): number {
    let max = 0
    for (const stamp of stamps.data?.result ?? []) {
      const r = parseStampRect(stamp.rect)
      if (r && r.layer > max) max = r.layer
    }
    return max
  }

  /** The lowest `rect.layer` currently in use among this page's own rect stamps (`0` if there are
   * none yet) — the send-to-back counterpart of `maxLayerOnPage` above. */
  function minLayerOnPage(): number {
    let min = 0
    for (const stamp of stamps.data?.result ?? []) {
      const r = parseStampRect(stamp.rect)
      if (r && r.layer < min) min = r.layer
    }
    return min
  }

  async function openEditorAndCreate(point: { x: number; y: number }, rect: StampRect | null) {
    onPlaced()
    const result = await stampEditorDialog("", "", rect)
    if (result === null) return
    addStamp.mutate({
      page,
      content: result.content,
      icon: result.icon,
      position: `${point.x.toFixed(2)},${point.y.toFixed(2)}`,
      rect: result.rect ? formatStampRect(result.rect) : undefined,
    })
  }

  /** Opens the rename/re-icon/rect-style editor for an *existing* stamp — shared by the
   * right-click menu's own "Edit Marker" item and a rect stamp's icon double-click, so the two
   * entry points can't drift on what "edit" actually does. */
  async function openEditorForExisting(stampId: string) {
    const current = stamps.data?.result.find((s) => s.id === stampId)
    const currentRect = current ? parseStampRect(current.rect) : null
    const result = await stampEditorDialog(current?.content ?? "", current?.icon ?? "", currentRect)
    if (result === null) return
    updateStamp.mutate({
      stampId,
      content: result.content,
      icon: result.icon,
      // Only the anchor/color/fill/corner half of a rect can change from this dialog (the
      // geometry is adjusted via the resize handles instead) — re-sending the rect's own
      // already-current `x/y/width/height` alongside the edited fields, rather than sending
      // nothing, since `useUpdateStamp` treats an omitted `rect` as "leave it alone" but a
      // *plain* stamp genuinely has no rect to send (`result.rect` is `null` there, matching
      // `defaultRect`).
      rect: result.rect ? formatStampRect(result.rect) : undefined,
    })
    // `rectEdit` — once set for a given `stampId` by a real drag/resize, it's deliberately never
    // cleared (see its own docs above) and *always* wins over the server-fresh `storedRect` for
    // rendering that stamp. Editing this same stamp through the dialog instead (a completely
    // separate path that only ever touches the server + query cache, never this state) used to
    // leave that stale `rectEdit` in place, so the dialog's own edit — a real display-mode toggle,
    // a color change, whatever — would never actually render until something else (a page
    // reload) wiped the component's local state clean. Reported live twice, in both directions:
    // once for a display-mode change not taking effect without a reload, and again as "反过来也是"
    // (the reverse happens too — dragging first, then editing via the dialog, has the same
    // problem). Updating `rectEdit` here too, whenever the dialog actually changed the rect,
    // keeps it in sync with whichever path most recently touched this stamp's geometry/style,
    // instead of only ever tracking the drag-handlers' own writes.
    if (result.rect) setRectEdit({ stampId, rect: result.rect, handle: null })
  }

  // Legacy's real `addStamp()`/keydown handler (`~/LANraragi/public/js/mod/reader_stamps.js`,
  // verified against the current source, not the stale pre-rewrite clone an earlier pass here
  // mistakenly reasoned from) doesn't cover the page with a click-catching div at all — it bumps
  // the *image's own* `z-index` above `.focus-overlay`'s so the image stays fully visible and
  // clickable, sitting above the dimmed backdrop rather than dimmed along with everything else,
  // and binds the click handler directly to `.reader-image`. The z-index/cursor styling itself is
  // `Reader.tsx`'s job (see its own `imageStyle` — it owns that `<img>`'s JSX, so a plain
  // conditional style prop there is the idiomatic way to drive it, not this component reaching
  // into a ref to mutate `style` directly). This effect only does the one thing that genuinely
  // requires a ref + native listener — reacting to pointer events on a DOM node this component
  // doesn't render itself.
  //
  // A plain mousedown-then-mouseup with no real movement between (or below
  // `RECT_DRAG_THRESHOLD_PX`) places a point stamp exactly like before; a mousedown followed by a
  // real drag instead places a rectangle stamp anchored at the mousedown corner, opening the
  // editor with that rectangle attached once the button is released.
  useEffect(() => {
    const imgOrNull = imageRef.current
    if (!imgOrNull || !placementMode) return
    // Re-bound to a `const` of the narrowed (non-null) type, same reasoning as `start`'s own
    // re-binding further down — needed by the nested `onUp` closure below.
    const img: HTMLImageElement = imgOrNull

    function onMouseDown(e: MouseEvent) {
      if (e.button !== 0) return
      // See the click-handler docs above for why both of these are needed: `stopPropagation()`
      // keeps `#imgLink`'s own page-turn handler from firing, and since that's exactly the
      // handler that would have called `preventDefault()` to stop the anchor's native navigation,
      // this listener has to take over that responsibility itself.
      e.preventDefault()
      e.stopPropagation()
      const startPoint = percentFromEvent(e.clientX, e.clientY)
      if (!startPoint) return
      // Re-bound to a `const` of the narrowed (non-null) type — TypeScript's control-flow
      // narrowing of `startPoint` above doesn't survive into the nested closures below, since it
      // can't prove nothing reassigns it between now and whenever they actually run.
      const start: { x: number; y: number } = startPoint
      const startClientX = e.clientX
      const startClientY = e.clientY
      setPlacementDrag({ startX: start.x, startY: start.y, curX: start.x, curY: start.y })

      function onMove(moveEvent: MouseEvent) {
        const cur = percentFromEvent(moveEvent.clientX, moveEvent.clientY)
        if (!cur) return
        setPlacementDrag({ startX: start.x, startY: start.y, curX: cur.x, curY: cur.y })
      }

      function onUp(upEvent: MouseEvent) {
        window.removeEventListener("mousemove", onMove)
        window.removeEventListener("mouseup", onUp)
        // `mousedown`'s own `preventDefault()`/`stopPropagation()` above only ever applied to
        // that one event — completing a press-release sequence over the anchor makes the browser
        // fire a separate, later `click` event of its own, which is exactly what `#imgLink`'s own
        // React `onClick` (page-turn) listens for; suppressing `mousedown` does nothing to stop
        // it. A capturing, run-once `click` listener on `img` intercepts that one event
        // specifically (capture phase — fires before the click can bubble anywhere, including to
        // this same `img`'s own placement-click listener above, which is fine here since a drag
        // that moved should never *also* count as the plain-click placement path). Verified live:
        // finishing a rectangle drag (mouse released after real on-image movement) still turned
        // the page immediately after the editor dialog opened, because nothing suppressed that
        // separate `click` from ever reaching the anchor.
        img.addEventListener(
          "click",
          (clickEvent) => {
            clickEvent.preventDefault()
            clickEvent.stopPropagation()
          },
          { capture: true, once: true },
        )
        const distance = Math.hypot(upEvent.clientX - startClientX, upEvent.clientY - startClientY)
        setPlacementDrag(null)
        if (distance < RECT_DRAG_THRESHOLD_PX) {
          void openEditorAndCreate(start, null)
          return
        }
        const cur = percentFromEvent(upEvent.clientX, upEvent.clientY) ?? start
        const x = Math.min(start.x, cur.x)
        const y = Math.min(start.y, cur.y)
        const width = Math.abs(cur.x - start.x)
        const height = Math.abs(cur.y - start.y)
        void openEditorAndCreate(
          { x: x + width / 2, y: y + height / 2 },
          { x, y, width, height, layer: maxLayerOnPage() + 1, ...lastPickedRectStyle() },
        )
      }

      window.addEventListener("mousemove", onMove)
      window.addEventListener("mouseup", onUp)
    }

    img.addEventListener("mousedown", onMouseDown)
    return () => img.removeEventListener("mousedown", onMouseDown)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [placementMode])

  // Deliberately not a port of legacy's own drag handler (`reader_stamps.js`'s
  // `createMarkerElement`) — its `mousemove` sets the marker's position straight from the
  // cursor's own absolute coordinates, which only lines up with the marker if the mousedown
  // happened to land exactly on its center; anywhere else in its hit area (its edge, say), the
  // very first `mousemove` snaps the marker to the cursor's position outright — a real, visible
  // jump the instant a drag starts, not a porting bug introduced here. Tracking the *offset* from
  // the cursor's position at pointer-down instead (added to the marker's own starting position on
  // every move) keeps the marker under the cursor at whatever point it was actually grabbed,
  // moving smoothly from there with no snap.
  function handleMarkerPointerDown(e: React.MouseEvent<HTMLDivElement>, stampId: string, x: number, y: number) {
    if (e.button !== 0) return
    e.stopPropagation()
    draggedRef.current = false
    setActiveDragStampId(stampId)
    setDrag({ stampId, x, y })
    const startClientX = e.clientX
    const startClientY = e.clientY

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      draggedRef.current = true
      const rect = img.getBoundingClientRect()
      const deltaXPercent = ((moveEvent.clientX - startClientX) / rect.width) * 100
      const deltaYPercent = ((moveEvent.clientY - startClientY) / rect.height) * 100
      const nx = clampPercent(x + deltaXPercent)
      const ny = clampPercent(y + deltaYPercent)
      setDrag({ stampId, x: nx, y: ny })
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove)
      window.removeEventListener("mouseup", onUp)
      setActiveDragStampId(null)
      if (draggedRef.current) {
        setDrag((current) => {
          if (current) {
            updateStamp.mutate({
              stampId: current.stampId,
              position: `${current.x.toFixed(2)},${current.y.toFixed(2)}`,
            })
          }
          // Deliberately kept, not nulled — see `drag`'s own docs above.
          return current
        })
      }
    }

    window.addEventListener("mousemove", onMove)
    window.addEventListener("mouseup", onUp)
  }

  /** Commits a rect edit's final `x,y,width,height` (the anchor/color segments are left as they
   * were — only the geometry changes here) to the server, and moves the stamp's own plain
   * `position` point to the rect's new center, keeping the two in sync the same way a freshly
   * dragged rectangle's point is centered on it at creation time. */
  function commitRectEdit(stampId: string, rect: StampRect) {
    updateStamp.mutate({
      stampId,
      position: `${(rect.x + rect.width / 2).toFixed(2)},${(rect.y + rect.height / 2).toFixed(2)}`,
      rect: formatStampRect(rect),
    })
  }

  /** Dragging the rect's own body (not a handle, not the icon) moves the whole rectangle — same
   * offset-from-pointer-down tracking as `handleMarkerPointerDown` above, for the same
   * no-snap-on-grab reason, clamped so the rect can't be dragged partly off the page image.
   *
   * Holding Ctrl while the drag *starts* switches to copy mode instead — handled by a completely
   * separate code path (`handleRectCopyDragPointerDown` below) rather than branching partway
   * through this one: an earlier version shared `rectEdit` (the *original* stamp's own live
   * position) for both the move-in-place preview and the copy's own live "where it'll land"
   * preview, which meant the original's own rendered rect visually followed the cursor during a
   * copy-drag — there was never a separate copy of the outline left behind at the original spot,
   * the *same* one just moved. Reported live as "我看到的是复制源的选框随着复制被带走了（应该留下
   * 一个非编辑状态选框在原地的）." Keeping the original's own `rectEdit` completely untouched
   * during a copy-drag, and tracking the live "ghost" preview in `copyDragPreview` instead, is
   * what actually leaves the original's outline exactly where it started for the whole gesture. */
  function handleRectMovePointerDown(e: React.MouseEvent, stamp: StampJson, rect: StampRect) {
    if (e.button !== 0) return
    e.stopPropagation()
    if (e.ctrlKey) {
      handleRectCopyDragPointerDown(e, stamp, rect)
      return
    }
    const stampId = stamp.id
    rectEditedRef.current = false
    setRectEdit({ stampId, rect, handle: null })
    setActiveRectEditStampId(stampId)
    const startClientX = e.clientX
    const startClientY = e.clientY

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      rectEditedRef.current = true
      const imgRect = img.getBoundingClientRect()
      const deltaXPercent = ((moveEvent.clientX - startClientX) / imgRect.width) * 100
      const deltaYPercent = ((moveEvent.clientY - startClientY) / imgRect.height) * 100
      const x = Math.min(Math.max(rect.x + deltaXPercent, 0), 100 - rect.width)
      const y = Math.min(Math.max(rect.y + deltaYPercent, 0), 100 - rect.height)
      setRectEdit({ stampId, rect: { ...rect, x, y }, handle: null })
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove)
      window.removeEventListener("mouseup", onUp)
      setActiveRectEditStampId(null)
      if (!rectEditedRef.current) {
        setRectEdit((current) => (current && current.stampId === stampId ? null : current))
        return
      }
      setRectEdit((current) => {
        if (current) commitRectEdit(current.stampId, current.rect)
        return current
      })
    }

    window.addEventListener("mousemove", onMove)
    window.addEventListener("mouseup", onUp)
  }

  /** The Ctrl-held branch of `handleRectMovePointerDown` above — tracks the about-to-be-created
   * copy's own live position in `copyDragPreview`, a state entirely separate from `rectEdit`, so
   * the *original* stamp's own rendered rect/outline never moves even a pixel for the whole
   * gesture (see that function's own docs for why sharing `rectEdit` between the two was a real
   * bug). `stamp`/`rect` here are the original's own values at the moment the drag started —
   * frozen for the same reason `handleRectMovePointerDown`'s own closed-over `rect` is: the
   * dragged copy's geometry is always "the original's own width/height, positioned wherever the
   * cursor ends up," not something that should shift if the original itself were somehow edited
   * mid-drag by another path. */
  function handleRectCopyDragPointerDown(e: React.MouseEvent, stamp: StampJson, rect: StampRect) {
    const draggedRef = { current: false }
    setCopyDragPreview({ stampId: stamp.id, rect })
    // Captured immediately at mousedown (matching `handleRectMovePointerDown`'s own
    // `startClientX`/`startClientY`), not lazily on the first `onMove` call — a lazily-captured
    // "start" position is actually the *first move's own* position, making that first move's own
    // delta always `0,0` relative to itself. Harmless for a real mouse (many small move events
    // arrive before release, so the *next* move after the first still computes a real, if
    // slightly short, delta) but a real bug for anything that jumps straight from mousedown's
    // position to wherever the button was released in a single `mousemove` — verified live: the
    // copy's own dashed preview never actually moved from the original's position while dragging.
    const startClientX = e.clientX
    const startClientY = e.clientY

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      draggedRef.current = true
      const imgRect = img.getBoundingClientRect()
      const deltaXPercent = ((moveEvent.clientX - startClientX) / imgRect.width) * 100
      const deltaYPercent = ((moveEvent.clientY - startClientY) / imgRect.height) * 100
      const x = Math.min(Math.max(rect.x + deltaXPercent, 0), 100 - rect.width)
      const y = Math.min(Math.max(rect.y + deltaYPercent, 0), 100 - rect.height)
      setCopyDragPreview({ stampId: stamp.id, rect: { ...rect, x, y } })
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove)
      window.removeEventListener("mouseup", onUp)
      if (!draggedRef.current) {
        setCopyDragPreview(null)
        return
      }
      setCopyDragPreview((current) => {
        const draggedRect = current?.stampId === stamp.id ? current.rect : rect
        const copyRect = { ...draggedRect, layer: maxLayerOnPage() + 1 }
        addStamp.mutate(
          {
            page,
            content: stamp.content,
            icon: stamp.icon,
            position: `${(copyRect.x + copyRect.width / 2).toFixed(2)},${(copyRect.y + copyRect.height / 2).toFixed(2)}`,
            rect: formatStampRect(copyRect),
          },
          {
            // Moves edit mode onto the copy the instant it's created, rather than leaving the
            // *original* selected (or nothing selected at all) — the whole point of dragging out
            // a copy is to then keep adjusting the new one, not the one that was just left
            // behind. `rectEdit` is seeded here too (not just `selectedStampId`), so the copy
            // renders with its own already-known geometry/style immediately, the same
            // no-flash-of-stale-data reasoning `commitRectEdit`'s own callers rely on elsewhere
            // in this file — otherwise it'd render from whatever `stamps.data` had (nothing, for
            // one render) until the post-mutation refetch actually lands.
            onSuccess: (data) => {
              setCopyDragPreview(null)
              setSelectedStampId(data.stamp_id)
              setRectEdit({ stampId: data.stamp_id, rect: copyRect, handle: null })
            },
          },
        )
        // Kept showing (not cleared here) until the mutation's own `onSuccess` above actually
        // clears it — the gap between "mouse released" and "copy actually created" is always
        // nonzero, and the preview disappearing before the real copy exists would read as the
        // whole gesture having silently failed.
        return current
      })
    }

    window.addEventListener("mousemove", onMove)
    window.addEventListener("mouseup", onUp)
  }

  /** Dragging the icon itself (only possible once a rect stamp is selected — see the icon's own
   * `onMouseDown` below) moves it to whichever of the 8 anchor points is currently nearest the
   * cursor, snapping rather than following the cursor freely: the icon's position is *defined* as
   * one of those 8 points (`anchorOnRect`), so there's no other coordinate space for it to
   * meaningfully occupy. Nearest-anchor is picked by straight-line distance in on-screen pixels
   * (not percent) so the snap feels consistent regardless of the rect's own aspect ratio. */
  function handleIconAnchorDragPointerDown(e: React.MouseEvent, stampId: string, rect: StampRect) {
    if (e.button !== 0) return
    e.stopPropagation()
    rectEditedRef.current = false
    setRectEdit({ stampId, rect, handle: null })
    setActiveRectEditStampId(stampId)

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      rectEditedRef.current = true
      const imgRect = img.getBoundingClientRect()
      const cursorXPercent = ((moveEvent.clientX - imgRect.left) / imgRect.width) * 100
      const cursorYPercent = ((moveEvent.clientY - imgRect.top) / imgRect.height) * 100
      let nearest: StampAnchor = rect.anchor
      let nearestDist = Infinity
      for (const a of RESIZE_HANDLES) {
        const p = anchorPercent(a)
        const ax = rect.x + (rect.width * p.x) / 100
        const ay = rect.y + (rect.height * p.y) / 100
        // Percent-space deltas converted to real pixels (`* imgRect.width/height`) before
        // comparing — otherwise a wide-but-short rect would bias distance comparisons toward
        // whichever axis has the larger percent-per-pixel ratio, picking a visually-further
        // anchor as "nearest" just because its percent-space distance happened to be smaller.
        const dx = (cursorXPercent - ax) * imgRect.width
        const dy = (cursorYPercent - ay) * imgRect.height
        const dist = dx * dx + dy * dy
        if (dist < nearestDist) {
          nearestDist = dist
          nearest = a
        }
      }
      setRectEdit({ stampId, rect: { ...rect, anchor: nearest }, handle: null })
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove)
      window.removeEventListener("mouseup", onUp)
      setActiveRectEditStampId(null)
      if (rectEditedRef.current) {
        setRectEdit((current) => {
          if (current) commitRectEdit(current.stampId, current.rect)
          return current
        })
      }
    }

    window.addEventListener("mousemove", onMove)
    window.addEventListener("mouseup", onUp)
  }

  /** Minimum rect size (percent of the page image) a resize can shrink to — prevents a handle
   * drag from collapsing the rectangle to nothing, which would leave no visible outline left to
   * grab again afterward. */
  const MIN_RECT_SIZE = 2

  /** Dragging one of the 8 resize handles moves only that corner/edge, keeping the opposite one
   * fixed — the standard resize-handle behavior. A corner handle (`tl`/`tr`/`br`/`bl`) adjusts
   * both axes; an edge midpoint (`t`/`r`/`b`/`l`) adjusts only the axis it sits on.
   *
   * Holding Shift locks the rect's own on-screen (pixel) aspect ratio while resizing — read live
   * from `moveEvent.shiftKey` on every move (not captured once at mousedown) so toggling Shift
   * mid-drag takes effect immediately, matching how every other app with this convention (e.g.
   * image editors) behaves. The lock is computed in *pixels*, not the `x`/`y`/`width`/`height`
   * percent fields directly — those are percentages of the image's own width and height
   * separately, which aren't the same physical scale unless the image happens to be square, so
   * naively keeping `width% === height% * ratio` would visibly distort on any non-square page. */
  function handleResizeHandlePointerDown(e: React.MouseEvent, stampId: string, rect: StampRect, handle: StampAnchor) {
    if (e.button !== 0) return
    e.stopPropagation()
    rectEditedRef.current = false
    setRectEdit({ stampId, rect, handle })
    setActiveRectEditStampId(stampId)
    const startClientX = e.clientX
    const startClientY = e.clientY
    const right = rect.x + rect.width
    const bottom = rect.y + rect.height
    const affectsLeft = handle === "tl" || handle === "l" || handle === "bl"
    const affectsRight = handle === "tr" || handle === "r" || handle === "br"
    const affectsTop = handle === "tl" || handle === "t" || handle === "tr"
    const affectsBottom = handle === "bl" || handle === "b" || handle === "br"

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      rectEditedRef.current = true
      const imgRect = img.getBoundingClientRect()
      const deltaXPercent = ((moveEvent.clientX - startClientX) / imgRect.width) * 100
      const deltaYPercent = ((moveEvent.clientY - startClientY) / imgRect.height) * 100

      let { x, y, width, height } = rect

      if (affectsLeft) {
        x = clampPercent(Math.min(rect.x + deltaXPercent, right - MIN_RECT_SIZE))
        width = right - x
      } else if (affectsRight) {
        const newRight = clampPercent(Math.max(right + deltaXPercent, rect.x + MIN_RECT_SIZE))
        width = newRight - x
      }
      if (affectsTop) {
        y = clampPercent(Math.min(rect.y + deltaYPercent, bottom - MIN_RECT_SIZE))
        height = bottom - y
      } else if (affectsBottom) {
        const newBottom = clampPercent(Math.max(bottom + deltaYPercent, rect.y + MIN_RECT_SIZE))
        height = newBottom - y
      }

      if (moveEvent.shiftKey) {
        // Pixel aspect ratio of the rect as it was when the drag started — locked for the whole
        // gesture (not recomputed mid-drag), same as every other app's own Shift-resize.
        const pixelRatio = (rect.width * imgRect.width) / (rect.height * imgRect.height)
        const onlyHorizontal = affectsLeft || affectsRight
        const onlyVertical = affectsTop || affectsBottom
        // An edge handle only has one free axis to begin with — deriving the *other*, currently
        // fixed axis from it (rather than trying to adjust the dragged axis itself, which would
        // fight the user's own cursor position) is the only way "lock ratio" means anything for
        // that handle. A corner handle has both axes free, so it instead follows whichever axis
        // the cursor actually moved further along in real pixels — matches the felt sense of
        // "I'm dragging this corner out" rather than one axis always silently winning regardless
        // of which way the mouse actually moved.
        const growHorizontally = onlyVertical
          ? false
          : onlyHorizontal || Math.abs(moveEvent.clientX - startClientX) >= Math.abs(moveEvent.clientY - startClientY)
        if (growHorizontally) {
          const widthPx = (width / 100) * imgRect.width
          height = (widthPx / pixelRatio / imgRect.height) * 100
        } else {
          const heightPx = (height / 100) * imgRect.height
          width = (heightPx * pixelRatio / imgRect.width) * 100
        }
        // Re-pin whichever edge(s) weren't actually being dragged back to the original rect's own
        // fixed corner/edge — a corner drag adjusts both axes together, so both need re-pinning;
        // an edge drag only ever touches one axis to begin with, so the other was never moved off
        // `rect.x`/`rect.y` in the first place and re-pinning it here is a no-op for that case.
        x = affectsLeft ? right - width : rect.x
        y = affectsTop ? bottom - height : rect.y
        // The non-Shift branch above clamps each edge to the image's own 0-100 bounds as it
        // computes it, but the ratio lock can push the *derived* axis past those same bounds even
        // though the dragged axis alone would have stayed within them (e.g. a very wide source
        // rect resized taller under lock quickly runs its derived width off either edge) — clamp
        // width/height (shrinking, not just clipping position, so the ratio itself stays intact)
        // against whichever edge is actually fixed for this handle.
        if (x < 0) {
          width += x
          x = 0
        }
        if (x + width > 100) width = 100 - x
        if (y < 0) {
          height += y
          y = 0
        }
        if (y + height > 100) height = 100 - y
        width = Math.max(width, MIN_RECT_SIZE)
        height = Math.max(height, MIN_RECT_SIZE)
      }

      setRectEdit({ stampId, rect: { ...rect, x, y, width, height }, handle })
    }

    function onUp() {
      window.removeEventListener("mousemove", onMove)
      window.removeEventListener("mouseup", onUp)
      setActiveRectEditStampId(null)
      if (rectEditedRef.current) {
        setRectEdit((current) => {
          if (current) commitRectEdit(current.stampId, current.rect)
          return current
        })
      }
    }

    window.addEventListener("mousemove", onMove)
    window.addEventListener("mouseup", onUp)
  }

  // Keyboard shortcuts for whichever stamp is currently selected (edit mode — see the effect's
  // own guard below) — T/B reorder it to the front/back of this page's own stacking order, Delete
  // removes it outright, Enter opens the same rename/re-icon/rect-style editor a double-click on
  // the icon does, the arrow keys nudge a selected rect stamp by 1 screen pixel per press.
  //
  // Depends on `stamps.data` and `rectEdit` in addition to `selectedStampId` — without the former,
  // this effect (and the `onKeyDown` closure it creates) only ever re-runs when *selection itself*
  // changes, so `stamp`/`rect`/`maxLayerOnPage()`/`minLayerOnPage()` inside stayed frozen at
  // whatever `stamps.data` was at the moment the stamp was selected, not whatever it actually is
  // by the time a key is pressed. Verified live: pressing T twice in a row on the same
  // still-selected stamp sent the exact same `layer` value to the server both times —
  // `maxLayerOnPage()` was reading a snapshot from *before* the first press's own update had
  // landed in the query cache, so "one more than the current max" kept computing the same stale
  // answer instead of ratcheting up each time. `rectEdit` is needed for the identical reason on
  // the arrow-key branch specifically — it reads `rectEdit` to prefer the most up-to-date known
  // geometry over the (possibly not-yet-refetched) server value, which only actually works if the
  // closure is rebuilt every time `rectEdit` itself changes, i.e. after every single nudge.
  useEffect(() => {
    if (!selectedStampId) return
    function onKeyDown(e: KeyboardEvent) {
      // Bails out of every branch below while focus is inside a real text input/textarea
      // elsewhere on the page (e.g. typing "delete" or "t" into the stamp name field of the very
      // dialog this effect can itself open) — none of these shortcuts should fire from ordinary
      // typing just because a stamp happens to still be selected underneath an open dialog.
      const active = document.activeElement
      if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) return
      if (!selectedStampId) return
      const stamp = stamps.data?.result.find((s) => s.id === selectedStampId)
      const storedRect = stamp ? parseStampRect(stamp.rect) : null
      // Prefers the live `rectEdit` (if it's for this same stamp) over the server-stored rect, for
      // the same reason every pointer-drag handler in this file already does — `rectEdit` is the
      // most up-to-date known geometry, ahead of whatever the PUT-then-refetch round trip has
      // actually confirmed yet. Without this, pressing an arrow key twice in quick succession
      // (well within one commit's own round trip) would compute the *second* nudge's delta from
      // the *pre-first-press* position, silently dropping the first press's own movement.
      const rect = rectEdit?.stampId === selectedStampId ? rectEdit.rect : storedRect
      switch (e.key) {
        case "t":
        case "T":
          if (!stamp || !rect) return
          e.preventDefault()
          // `b`/`B` below collides with `Reader.tsx`'s own bookmark-toggle shortcut, `Backspace`
          // with its own "back to library" shortcut — both listeners are attached via
          // `window.addEventListener('keydown', ...)` on the *same* target, so without this, a
          // selected stamp still let *both* handlers act on the same keypress (e.g. deleting the
          // stamp *and* navigating back to the library on one Backspace). `stopImmediatePropagation`
          // (not just `stopPropagation`, which only stops bubbling to *ancestors* and does nothing
          // for another listener on this same target) is what actually suppresses that sibling
          // listener. Applied to every branch here, not only the ones with a real same-key
          // collision today — a shortcut a stamp is actively selected for editing should always
          // take exclusive ownership of that keypress, not depend on remembering which specific
          // keys currently happen to collide with `Reader.tsx`'s own bindings.
          e.stopImmediatePropagation()
          updateStamp.mutate({ stampId: stamp.id, rect: formatStampRect({ ...rect, layer: maxLayerOnPage() + 1 }) })
          return
        case "b":
        case "B":
          if (!stamp || !rect) return
          e.preventDefault()
          e.stopImmediatePropagation()
          updateStamp.mutate({ stampId: stamp.id, rect: formatStampRect({ ...rect, layer: minLayerOnPage() - 1 }) })
          return
        case "Delete":
        case "Backspace":
          e.preventDefault()
          e.stopImmediatePropagation()
          setSelectedStampId(null)
          deleteStamp.mutate(selectedStampId)
          return
        case "Enter":
          e.preventDefault()
          e.stopImmediatePropagation()
          void openEditorForExisting(selectedStampId)
          return
        case "ArrowLeft":
        case "ArrowRight":
        case "ArrowUp":
        case "ArrowDown": {
          if (!stamp || !rect) return
          const img = imageRef.current
          if (!img) return
          e.preventDefault()
          e.stopImmediatePropagation()
          const imgRect = img.getBoundingClientRect()
          // Requested explicitly as a 1-screen-pixel step — converted to this rect's own percent
          // units via the rendered image's actual size, same conversion every pointer-drag handler
          // in this file already does for mouse deltas, just with a fixed 1px delta instead of a
          // live cursor position.
          const stepXPercent = (1 / imgRect.width) * 100
          const stepYPercent = (1 / imgRect.height) * 100
          const stampId = stamp.id
          // Functional `setRectEdit` update — reads whatever `rectEdit` React actually has queued
          // at apply time, not the `rect` this closure captured back when the *effect* was last
          // rebuilt. Necessary for OS key-repeat: holding an arrow key fires many keydown events
          // in a tight burst, several of which can land inside the same React batch before a
          // single one of those `setRectEdit` calls has actually been applied yet — verified live
          // (5 synthetic keydowns dispatched back-to-back in one script tick all computed their
          // delta from the same starting `rect`, so the rect only moved 1 step total instead of
          // 5) that using the closed-over `rect` here silently drops every nudge but the first
          // one in a burst rather than accumulating them.
          setRectEdit((current) => {
            const base = current?.stampId === stampId ? current.rect : rect
            let { x, y } = base
            // Clamped against `100 - base.width`/`100 - base.height` (the rect's own size-aware
            // bound), not a plain 0-100 `clampPercent`, for the same reason every pointer-drag
            // handler in this file clamps that way — a plain 0-100 clamp on `x` alone would still
            // let the rect's own *right edge* run off the image once `x + width > 100`.
            switch (e.key) {
              case "ArrowLeft":
                x = Math.max(x - stepXPercent, 0)
                break
              case "ArrowRight":
                x = Math.min(x + stepXPercent, 100 - base.width)
                break
              case "ArrowUp":
                y = Math.max(y - stepYPercent, 0)
                break
              case "ArrowDown":
                y = Math.min(y + stepYPercent, 100 - base.height)
                break
            }
            const nextRect = { ...base, x, y }
            if (arrowNudgeCommitTimeout.current) clearTimeout(arrowNudgeCommitTimeout.current)
            arrowNudgeCommitTimeout.current = setTimeout(() => {
              commitRectEdit(stampId, nextRect)
            }, 400)
            return { stampId, rect: nextRect, handle: null }
          })
          return
        }
        default:
      }
    }
    // Capture phase (the trailing `true`), not the default bubble phase — `stopImmediatePropagation`
    // in the arrow-key branch above only blocks *later-registered* listeners on the same `window`
    // target, and relying on "this effect's listener happens to register before `Reader.tsx`'s own"
    // is not something React actually guarantees deterministically (verified live: it does NOT hold
    // in practice — a real ArrowRight keypress with a rect stamp selected still flipped the page,
    // meaning `Reader.tsx`'s bubble-phase listener was running *first* despite `MarkerLayer` being
    // the child component). Capture-phase listeners on `window` always run before *any* bubble-phase
    // listener anywhere in the tree, independent of registration order — matches the same fix
    // `ArchiveOverviewOverlay.tsx` already uses for its own arrow-key handling, for the identical
    // "must reliably win over `Reader.tsx`'s own page-nav keydown" reason.
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedStampId, stamps.data, rectEdit])

  // Cancels a still-pending debounced arrow-nudge commit (see `arrowNudgeCommitTimeout`'s own
  // docs above) on unmount only — deliberately its own effect with an empty dependency array
  // rather than folded into the keyboard-shortcuts effect above, since that one's own cleanup
  // already runs on every `rectEdit` change (i.e. after every single nudge, by design) and doing
  // it there too would cancel the very timeout that same nudge just scheduled.
  useEffect(() => {
    return () => {
      if (arrowNudgeCommitTimeout.current) clearTimeout(arrowNudgeCommitTimeout.current)
    }
  }, [])

  if (!visible) return null

  return (
    <>
      {/* While a rect stamp is in edit mode, a real full-viewport `<div>` — not an event-listener
          trick — sits between the page image and the stamps layer (z-index between the image's
          effective paint order and `.marker`'s own hardcoded `23`) so that ANY click on
          content this component doesn't own (the image, header, paginator, …) physically can't
          reach its real target at all; it hits this div first and only this div's own `onClick`
          (deselect) ever runs. Simpler and more robust than intercepting `mousedown`/`click` at
          the `window` level via capture-phase listeners (an earlier version of this fix): that
          approach raced its own state update — calling `setSelectedStampId(null)` inside the
          `mousedown` handler tore the listener down (via its own `useEffect` dependency on
          `selectedStampId`) before the browser's subsequent `click` event for that same physical
          click arrived, so the click still fell through to `#imgLink`'s own page-turn handler
          after all. A real DOM element in front of the click's target has no such race — there's
          nothing to "arrive too late" for, since the browser's own hit-testing is what's doing
          the blocking, not a listener with its own separate lifecycle. Each individual stamp's
          own rect/icon/handles still receive clicks normally since they render *after* this
          overlay in DOM order at a higher z-index (`23`, `.marker`'s own value) — this overlay
          only ever intercepts clicks that would otherwise have reached something *else*. */}
      {selectedStampId && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 20, cursor: "default" }}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => setSelectedStampId(null)}
        />
      )}
      {/* Every `.marker`'s own `left`/`top` `%` resolves against *this* box now — sized and
          positioned to match the real `<img>` exactly (see `imgBounds`'s own docs) — instead of
          whatever this component's own DOM-tree ancestor happened to be. `imgBounds` starts out
          `null` for the one render before the sizing effect's first measurement lands; skipping
          markers entirely for that one frame (rather than rendering them against a guessed size)
          avoids a visible flash at the wrong position before snapping to the right one. */}
      {/* Always mounted (not gated on `imgBounds` being measured yet) — `wrapperRef` needs to
          already point at a real DOM node the very first time the sizing effect above runs, since
          that effect reads `wrapperRef.current.offsetParent` to compute `imgBounds` in the first
          place; gating this on `imgBounds` itself would mean it can never become non-`null`.
          Positioned at `0,0`/zero-size until the first real measurement lands (rather than at a
          guessed size) — its children are only ever rendered once `imgBounds` is actually known
          (see the `imgBounds &&` gate around the stamps `.map()` below), so there's nothing at the
          wrong position to flash during that one gap frame. */}
      <div
        ref={wrapperRef}
        style={{
          position: "absolute",
          left: imgBounds?.left ?? 0,
          top: imgBounds?.top ?? 0,
          width: imgBounds?.width ?? 0,
          height: imgBounds?.height ?? 0,
          // Only the actual markers (each with their own `pointerEvents: 'auto'` below) should
          // ever intercept a click — the rest of this box is otherwise-empty space overlaying
          // the image, and shouldn't swallow clicks meant for the image itself (page-turning,
          // or — while `placementMode` is armed — placing a new stamp).
          pointerEvents: "none",
          // `.marker`'s own real CSS (`lrr.css`) already hardcodes `z-index: 23`, which is why a
          // plain point marker's icon has always painted above the page image without this
          // wrapper needing its own z-index — but the rect outline/resize-handle `<div>`s rendered
          // here are plain, unstyled elements with no such rule of their own. Verified live:
          // `elementsFromPoint` at the outline's own on-screen center returned `#img` *above* it
          // despite correct DOM order (this wrapper renders after `#imgLink` as a sibling, which
          // should already put it on top per normal stacking) — reported live as "hover图标时没显示
          // 选框" (hovering the icon doesn't show the outline): the outline element existed with
          // the right geometry and styling the entire time, just genuinely painted underneath the
          // image. Matching `.marker`'s own `23` here keeps every element in this layer consistent
          // with the one that already reliably works.
          zIndex: 23,
        }}
      >
        {imgBounds &&
          // Sorted by `rect.layer` ascending (a plain point stamp — no `rect` — sorts as `0`,
          // same as every rect stamp's own default) so a higher layer actually paints on top:
          // every stamp here shares the one `zIndex: 23` on this whole wrapper, so relative
          // stacking among them is decided by DOM order alone, later siblings painting over
          // earlier ones — this sort is what makes that order match `layer` instead of just
          // whatever order the API happened to return them in.
          [...(stamps.data?.result ?? [])]
            .sort((a, b) => (parseStampRect(a.rect)?.layer ?? 0) - (parseStampRect(b.rect)?.layer ?? 0))
            .map((stamp) => {
            const [xStr, yStr] = stamp.position.split(",")
            const stored = { x: Number(xStr), y: Number(yStr) }
            if (Number.isNaN(stored.x) || Number.isNaN(stored.y)) return null
            const isDragging = activeDragStampId === stamp.id

            const storedRect = parseStampRect(stamp.rect)
            // The locally-tracked live edit takes over rendering the instant it starts (not just
            // while actively dragging) for the same double-jump reason `drag`'s own docs explain
            // above — the PUT-then-refetch round trip is always slower to reflect the same answer.
            const rect = rectEdit?.stampId === stamp.id ? rectEdit.rect : storedRect
            const isSelected = selectedStampId === stamp.id
            const isHovered = hoveredStampId === stamp.id
            const isRectEditing = activeRectEditStampId === stamp.id

            // A rect stamp's icon position is *derived* — the anchor point on the rect's own
            // border — not the plain `position` field (which only exists on a rect stamp to keep
            // its center in sync for legacy/non-rect call sites, e.g. `stampedPages`'s own
            // page-level lookup). A plain point stamp (no rect) still uses `position` directly,
            // exactly as before.
            const iconPos = rect ? anchorOnRect(rect) : (drag?.stampId === stamp.id ? drag : stored)

            return (
              <div key={stamp.id} data-stamp-id={stamp.id}>
                {/* The rectangle's own outline + translucent fill — reveals on hover *or*
                    selection (not only selection); dragging the rect body and the 8 resize
                    handles are both only live once *selected*, matching the requested state
                    machine: plain hover previews the outline only (nothing draggable yet), a
                    single click on the rect or the icon "opens" it for adjustment (handles
                    appear, rect body and icon both become draggable), and only a *double*-click
                    on the icon opens the full rename/re-icon/rect-style editor dialog. Deliberately
                    behind the icon in DOM order (rendered first) so the icon's own hit area stays
                    on top. */}
                {rect && (rect.display === "always" || isHovered || isSelected) && (
                  <div
                    onMouseDown={(e) => {
                      // Reset unconditionally, not only inside `handleRectMovePointerDown` (which
                      // only ever runs once already selected) — otherwise a `true` left over from
                      // an earlier *actual* drag would permanently block every future plain click
                      // from selecting this stamp again, since nothing else ever clears it back to
                      // `false` before the next click's own `onClick` guard below reads it.
                      // Verified live: after one rect-body drag, clicking to re-select the same
                      // stamp (having deselected it since) silently did nothing from then on.
                      rectEditedRef.current = false
                      if (!isSelected) return
                      handleRectMovePointerDown(e, stamp, rect)
                    }}
                    onClick={(e) => {
                      if (rectEditedRef.current) {
                        e.preventDefault()
                        return
                      }
                      setSelectedStampId(stamp.id)
                    }}
                    onMouseEnter={() => setHoveredStampId(stamp.id)}
                    onMouseLeave={() => !isRectEditing && setHoveredStampId(null)}
                    style={{
                      position: "absolute",
                      left: `${rect.x}%`,
                      top: `${rect.y}%`,
                      width: `${rect.width}%`,
                      height: `${rect.height}%`,
                      border: `2px solid ${rect.color}`,
                      borderRadius: rect.corner === "round" ? 12 : 0,
                      boxSizing: "border-box",
                      cursor: isSelected ? "move" : "pointer",
                      pointerEvents: "auto",
                      ...rectFillStyle(rect),
                    }}
                  >
                    {isSelected &&
                      RESIZE_HANDLES.map((h) => {
                        const p = anchorPercent(h)
                        return (
                          <div
                            key={h}
                            onMouseDown={(e) => handleResizeHandlePointerDown(e, stamp.id, rect, h)}
                            style={{
                              position: "absolute",
                              left: `${p.x}%`,
                              top: `${p.y}%`,
                              transform: "translate(-50%, -50%)",
                              width: 10,
                              height: 10,
                              boxSizing: "border-box",
                              border: `1px solid ${rect.color}`,
                              background: "white",
                              cursor: resizeCursor(h),
                              pointerEvents: "auto",
                            }}
                          />
                        )
                      })}
                  </div>
                )}
                {/* Wraps `.marker` itself (not a separate wrapping `<span>` around it) with a
                    themed `Tooltip` instead of a plain `title` attribute — the browser's native
                    tooltip can't be restyled and has its own multi-second show delay, unlike this
                    app's other icon-only tooltips. `wrapperStyle={{ position: 'static' }}` +
                    `anchor="cursor"` mirrors `ArchiveOverviewOverlay.tsx`'s identical fix for the
                    same underlying issue: `Tooltip`'s own default wrapper is `position: relative`,
                    which would silently become `.marker`'s *new* positioning containing block
                    (`.marker` itself is `position: absolute; left/top: <percent>` relative to the
                    page image) — without the override the pin's `left`/`top` percentages would
                    resolve against the wrapper's own tiny shrink-to-fit box instead of the real
                    page image, moving every marker to the wrong spot. With no `position: relative`/
                    `absolute` of its own, that `static` wrapper's bounding box also collapses
                    around nothing once `.marker` escapes into absolute layout, so the default
                    `anchor="element"` mode would place the bubble at the wrong spot too —
                    `anchor="cursor"` sidesteps needing a meaningful wrapper box at all. */}
                <Tooltip label={stamp.content} wrapperStyle={{ position: "static" }} anchor="cursor">
                  <div
                    className="marker"
                    style={{
                      left: `${iconPos.x}%`,
                      top: `${iconPos.y}%`,
                      cursor: rect ? (isSelected ? "grab" : "pointer") : isDragging ? "grabbing" : "grab",
                      pointerEvents: "auto",
                      // A custom icon replaces `.marker`'s own CSS `background-image` (the default
                      // favicon pin) entirely, rather than rendering on top of it — showing both at
                      // once would just look like visual noise, not a real combined icon.
                      ...(stamp.icon && { backgroundImage: "none" }),
                    }}
                    onMouseEnter={() => rect && setHoveredStampId(stamp.id)}
                    onMouseLeave={() => !isRectEditing && setHoveredStampId(null)}
                    // A rect stamp's icon is only draggable once selected — before that, a plain
                    // click on it should just select (not immediately start a drag on the same
                    // press). `draggedRef.current` is still reset here unconditionally for the
                    // point-stamp branch, for the reason its own docs below explain.
                    onMouseDown={(e) => {
                      if (rect) {
                        // Reset unconditionally, same reasoning as the rect body's own
                        // `onMouseDown` above — `rectEditedRef` left `true` by an earlier *actual*
                        // rect-body/handle/icon drag anywhere would otherwise permanently block
                        // `onClick`'s own guard below from ever selecting this icon again.
                        rectEditedRef.current = false
                        if (isSelected) handleIconAnchorDragPointerDown(e, stamp.id, rect)
                        return
                      }
                      // `draggedRef.current` is reset here unconditionally (not only inside
                      // `handleMarkerPointerDown`) — otherwise a rect stamp's icon could never be
                      // selected at all once `draggedRef` had ever been left `true` by an earlier
                      // *point* stamp's real drag anywhere on the page: the `onClick` guard below
                      // reads the same shared ref, so a stale `true` from a previous, entirely
                      // unrelated stamp's drag would silently swallow every future click on this
                      // stamp's icon, forever, with no drag on this icon ever having happened.
                      draggedRef.current = false
                      handleMarkerPointerDown(e, stamp.id, iconPos.x, iconPos.y)
                    }}
                    onClick={(e) => {
                      // A drag that actually moved the pin/rect shouldn't also re-trigger selection
                      // or open the rename prompt via a trailing click — mouseup after dragging
                      // still fires a click event.
                      if (draggedRef.current || rectEditedRef.current) {
                        e.preventDefault()
                        return
                      }
                      if (rect) setSelectedStampId(stamp.id)
                    }}
                    // Only a double-click opens the full editor dialog — a single click is instead
                    // "select for adjustment" (handled by `onClick` above), matching the requested
                    // three-tier interaction (hover preview / single-click adjust / double-click
                    // edit) rather than single-click doing both at once.
                    onDoubleClick={() => {
                      void openEditorForExisting(stamp.id)
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault()
                      setMenu({ stampId: stamp.id, x: e.clientX, y: e.clientY })
                    }}
                  >
                    {stamp.icon && (
                      <span
                        style={{
                          position: "absolute",
                          inset: 0,
                          display: "flex",
                          alignItems: "center",
                          justifyContent: "center",
                          // `.marker`'s own real box is a fixed 24x24px (`lrr.css`) — 20px fills most
                          // of that (leaving a couple px of margin so the glyph doesn't visibly clip
                          // against the pin's own rounded silhouette) without needing to touch that
                          // box's real size, which stays shared with the default (no custom icon)
                          // favicon pin.
                          fontSize: 20,
                          lineHeight: 1,
                          pointerEvents: "none",
                        }}
                      >
                        {renderStampIcon(stamp.icon)}
                      </span>
                    )}
                  </div>
                </Tooltip>
              </div>
            )
          })}

          {/* Live preview of the rectangle currently being dragged out in placement mode — purely
              visual feedback while the mouse button is down; the actual stamp isn't created until
              `onUp` (see the placement-mode effect above) opens the editor dialog. `zIndex:
              Z_OVERLAY_CONTENT` is required here, not optional styling — `.focus-overlay` (below)
              is `position: fixed` with its own real `z-index: 21` (`lrr.css`), which otherwise
              paints over this preview (a plain `position: absolute` div with no explicit z-index
              of its own participates in normal stacking order, not above a sibling that opts into
              a stacking context via `z-index`) — verified live: the dashed box was completely
              invisible while dragging until this was added, exactly like the image itself needed
              raising above this same overlay for its own click-to-place handler to feel visible. */}
          {placementDrag && (
            <div
              style={{
                position: "absolute",
                left: `${Math.min(placementDrag.startX, placementDrag.curX)}%`,
                top: `${Math.min(placementDrag.startY, placementDrag.curY)}%`,
                width: `${Math.abs(placementDrag.curX - placementDrag.startX)}%`,
                height: `${Math.abs(placementDrag.curY - placementDrag.startY)}%`,
                // The last *confirmed* (submitted via the editor dialog's own 确定 button) rect
                // color, not a hardcoded red — this preview is drawn from the same
                // `lastPickedRectStyle()` the rect itself gets seeded with once the drag finishes
                // and `openEditorAndCreate` opens (line ~393 above); leaving the live preview
                // hardcoded red made it visibly disagree with the color the new rect was actually
                // about to be created with, reported live from a screenshot showing a red dashed
                // preview next to already-confirmed rects in a different (blue/purple) color.
                border: `2px dashed ${lastPickedRectStyle().color}`,
                background: `${lastPickedRectStyle().color}33`,
                boxSizing: "border-box",
                pointerEvents: "none",
                zIndex: Z_OVERLAY_CONTENT,
              }}
            />
          )}

          {/* The live "where will the copy land" preview for a Ctrl+drag rect copy — a *second*,
              independent rect from the original's own (which stays exactly where it started the
              whole time — see `handleRectCopyDragPointerDown`'s own docs). Dashed, in the source
              rect's own color, to read as "not yet real" the same way the placement-drag preview
              above does, rather than looking like an already-committed second stamp. */}
          {copyDragPreview && (
            <div
              style={{
                position: "absolute",
                left: `${copyDragPreview.rect.x}%`,
                top: `${copyDragPreview.rect.y}%`,
                width: `${copyDragPreview.rect.width}%`,
                height: `${copyDragPreview.rect.height}%`,
                border: `2px dashed ${copyDragPreview.rect.color}`,
                borderRadius: copyDragPreview.rect.corner === "round" ? 12 : 0,
                background: `${copyDragPreview.rect.color}33`,
                boxSizing: "border-box",
                pointerEvents: "none",
                zIndex: Z_OVERLAY_CONTENT,
              }}
            />
          )}
      </div>

      {/* Only present while marker-placement mode is armed (`S` key) — dims everything *except*
          the reader image (see the effect above, which raises just that element's own z-index
          above this one's), matching legacy's real `#overlay-page.focus-overlay` — this element
          itself is a pure backdrop now, not the click target; the click that actually places a
          stamp is bound directly to the image. */}
      {placementMode && <div className="focus-overlay" style={{ display: "block" }} />}

      {menu && (
        <>
          <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={() => setMenu(null)} />
          <PopupMenu style={{ position: "fixed", top: menu.y, left: menu.x, zIndex: Z_OVERLAY_CONTENT }}>
            <PopupMenuItem
              onClick={() => {
                const stampId = menu.stampId
                setMenu(null)
                void openEditorForExisting(stampId)
              }}
            >
              {t("reader.editMarker")}
            </PopupMenuItem>
            <PopupMenuItem
              onClick={() => {
                setMenu(null)
                deleteStamp.mutate(menu.stampId)
              }}
            >
              {t("reader.deleteMarker")}
            </PopupMenuItem>
          </PopupMenu>
        </>
      )}
    </>
  )
}

function clampPercent(n: number): number {
  return Math.min(Math.max(n, 0), 100)
}

/** A rect stamp's icon position — the given anchor point on the rect's own border, converted
 * from `anchorPercent`'s rect-relative 0-100 into the same page-relative percent space
 * `position`/plain markers already use. */
function anchorOnRect(rect: StampRect): { x: number; y: number } {
  const p = anchorPercent(rect.anchor)
  return {
    x: rect.x + (rect.width * p.x) / 100,
    y: rect.y + (rect.height * p.y) / 100,
  }
}

/** CSS `cursor` for a resize handle, matching the direction it actually resizes in — the usual
 * 8-way resize-cursor convention (`nwse-resize` for a corner running diagonally, `ns-resize`/
 * `ew-resize` for an edge midpoint running along one axis only). */
function resizeCursor(handle: StampAnchor): string {
  switch (handle) {
    case "tl":
    case "br":
      return "nwse-resize"
    case "tr":
    case "bl":
      return "nesw-resize"
    case "t":
    case "b":
      return "ns-resize"
    case "r":
    case "l":
      return "ew-resize"
  }
}

/** `background`/`backdropFilter` for a rect's own interior, one pair per `StampFill`. `solid`/
 * `stripes` are plain translucent overlays in the rect's own color — `stripes` a 45deg
 * `repeating-linear-gradient` (`rect.color` is a plain `#rrggbb` with no alpha channel, so opacity
 * is layered on via the `66`/`00` hex-alpha suffixes on each gradient stop instead), fixed 8px
 * band spacing/angle, not user-adjustable, matching what was actually requested (a fill *style*
 * choice, not a fully parameterized pattern editor). `mosaic`/`blur` instead genuinely obscure the
 * page content *underneath* the rect via `backdrop-filter: blur()` at two different intensities —
 * there's no real CSS filter that pixelates arbitrary backdrop content the way an image-editing
 * tool's mosaic effect does, so `mosaic` approximates the same "can't make out the underlying
 * detail" goal with a stronger blur radius instead of true square-block pixelation (confirmed
 * expectation-wise before implementing — an actual pixelation filter would need capturing the
 * underlying pixels via canvas, real added complexity/browser-compat risk for a purely visual
 * distinction from `blur`). Both still keep a faint tint of `rect.color` on top (much lower
 * opacity than `solid`) so the rect's own presence/color is still legible even where it's mostly
 * just blurring what's behind it. */
function rectFillStyle(rect: StampRect): { background: string; backdropFilter?: string } {
  switch (rect.fill) {
    case "solid":
      return { background: `${rect.color}33` }
    case "stripes":
      return {
        background: `repeating-linear-gradient(45deg, ${rect.color}66 0, ${rect.color}66 4px, ${rect.color}00 4px, ${rect.color}00 8px)`,
      }
    case "mosaic":
      return { background: `${rect.color}1a`, backdropFilter: "blur(14px)" }
    case "blur":
      return { background: `${rect.color}1a`, backdropFilter: "blur(6px)" }
  }
}
