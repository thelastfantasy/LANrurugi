import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useAddStamp, useDeleteStamp, useStampsForPage, useUpdateStamp } from "@/api/hooks"
import type { StampJson } from "@/api/types"
import { PopupMenu, PopupMenuItem, StaticTooltip, Tooltip, type TooltipAlign, type TooltipSide } from "@/components/common-ui/Display"
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
import { useSupportsHover } from "@/hooks/useSupportsHover"
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

/** Legacy stamp/marker feature: click places a pin, drag repositions, right-click edits/deletes;
 * a dragged placement carries a selection rectangle. */

/** Minimum drag (px) before a placement drag counts as a rectangle instead of a click. */
const RECT_DRAG_THRESHOLD_PX = 4

/** Rectangle resize handles, matching `StampAnchor`. */
const RESIZE_HANDLES: StampAnchor[] = ["tl", "t", "tr", "r", "br", "b", "bl", "l"]

interface ContextMenuState {
  stampId: string
  x: number
  y: number
}

/** A drag in progress: `stampId` and its live position, committed to the server on pointer-up. */
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
  loggedIn,
}: {
  archiveId: string
  page: number
  imageRef: React.RefObject<HTMLImageElement | null>
  visible: boolean
  /** True while the user has pressed `S` and is about to click a spot to drop a pin. */
  placementMode: boolean
  onPlaced: () => void
  /** Gates every stamp write path for guests. */
  loggedIn: boolean
}) {
  const { t } = useTranslation()
  const stamps = useStampsForPage(archiveId, page)
  const addStamp = useAddStamp(archiveId)
  const updateStamp = useUpdateStamp()
  const deleteStamp = useDeleteStamp()
  const supportsHover = useSupportsHover()
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [imgBounds, setImgBounds] = useState<{ left: number; top: number; width: number; height: number } | null>(null)
  useEffect(() => {
    const img = imageRef.current
    if (!img) return
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
    const rafId = requestAnimationFrame(updateBounds)
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
  const [drag, setDrag] = useState<DragState | null>(null)
  const [activeDragStampId, setActiveDragStampId] = useState<string | null>(null)
  const draggedRef = useRef(false)

  const [hoveredStampId, setHoveredStampId] = useState<string | null>(null)
  const [selectedStampId, setSelectedStampId] = useState<string | null>(null)
  const [rectEdit, setRectEdit] = useState<{ stampId: string; rect: StampRect; handle: StampAnchor | null } | null>(
    null,
  )
  const rectEditedRef = useRef(false)
  const arrowNudgeCommitTimeout = useRef<ReturnType<typeof setTimeout> | null>(null)
  const [activeRectEditStampId, setActiveRectEditStampId] = useState<string | null>(null)

  const [copyDragPreview, setCopyDragPreview] = useState<{ stampId: string; rect: StampRect } | null>(null)

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

  /** Highest rect.layer on this page (0 if none) — puts a new rectangle on top by default. */
  function maxLayerOnPage(): number {
    let max = 0
    for (const stamp of stamps.data?.result ?? []) {
      const r = parseStampRect(stamp.rect)
      if (r && r.layer > max) max = r.layer
    }
    return max
  }

  /** Lowest rect.layer on this page (0 if none) — send-to-back counterpart. */
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

  /** Editor opener shared by the context menu's Edit item and the icon double-click. */
  async function openEditorForExisting(stampId: string) {
    const current = stamps.data?.result.find((s) => s.id === stampId)
    const currentRect = current ? parseStampRect(current.rect) : null
    const result = await stampEditorDialog(current?.content ?? "", current?.icon ?? "", currentRect)
    if (result === null) return
    updateStamp.mutate({
      stampId,
      content: result.content,
      icon: result.icon,
      rect: result.rect ? formatStampRect(result.rect) : undefined,
    })
    // The dialog must sync `rectEdit` too — the never-cleared drag state would otherwise mask
    // dialog-side rect edits until a full reload.
    if (result.rect) setRectEdit({ stampId, rect: result.rect, handle: null })
  }

  // Placement mode: a plain click (or sub-threshold drag) places a point stamp; a real drag
  // places a rectangle anchored at the mousedown corner.
  useEffect(() => {
    const imgOrNull = imageRef.current
    if (!imgOrNull || !placementMode) return
    const img: HTMLImageElement = imgOrNull

    function onPointerDown(e: PointerEvent) {
      if (e.pointerType === "mouse" && e.button !== 0) return
      e.preventDefault()
      e.stopPropagation()
      const startPoint = percentFromEvent(e.clientX, e.clientY)
      if (!startPoint) return
      const start: { x: number; y: number } = startPoint
      const startClientX = e.clientX
      const startClientY = e.clientY
      const pointerId = e.pointerId
      img.setPointerCapture(pointerId)
      setPlacementDrag({ startX: start.x, startY: start.y, curX: start.x, curY: start.y })

      function onMove(moveEvent: PointerEvent) {
        if (moveEvent.pointerId !== pointerId) return
        const cur = percentFromEvent(moveEvent.clientX, moveEvent.clientY)
        if (!cur) return
        setPlacementDrag({ startX: start.x, startY: start.y, curX: cur.x, curY: cur.y })
      }

      function onUp(upEvent: PointerEvent) {
        if (upEvent.pointerId !== pointerId) return
        img.removeEventListener("pointermove", onMove)
        img.removeEventListener("pointerup", onUp)
        img.removeEventListener("pointercancel", onUp)
        // pointerdown's preventDefault doesn't stop the separate later `click` (#imgLink's
        // page-turn) — intercept that one click via a capture-phase, run-once listener.
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

      img.addEventListener("pointermove", onMove)
      img.addEventListener("pointerup", onUp)
      img.addEventListener("pointercancel", onUp)
    }

    img.addEventListener("pointerdown", onPointerDown)
    return () => img.removeEventListener("pointerdown", onPointerDown)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [placementMode])

  function handleMarkerPointerDown(e: React.PointerEvent<HTMLDivElement>, stampId: string, x: number, y: number) {
    if (e.pointerType === "mouse" && e.button !== 0) return
    e.stopPropagation()
    draggedRef.current = false
    setActiveDragStampId(stampId)
    setDrag({ stampId, x, y })
    const startClientX = e.clientX
    const startClientY = e.clientY
    const pointerId = e.pointerId
    e.currentTarget.setPointerCapture(pointerId)

    function onMove(moveEvent: PointerEvent) {
      if (moveEvent.pointerId !== pointerId) return
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

    function onUp(upEvent: PointerEvent) {
      if (upEvent.pointerId !== pointerId) return
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
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

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
  }

  /** Commits the rect edit and moves the stamp's plain position to the rect's new center. */
  function commitRectEdit(stampId: string, rect: StampRect) {
    updateStamp.mutate({
      stampId,
      position: `${(rect.x + rect.width / 2).toFixed(2)},${(rect.y + rect.height / 2).toFixed(2)}`,
      rect: formatStampRect(rect),
    })
  }

  /** Dragging the rect body moves the whole rect, clamped to the page image. Ctrl at drag start
   * switches to a separate copy path below so the original's outline never moves. */
  function handleRectMovePointerDown(e: React.PointerEvent, stamp: StampJson, rect: StampRect) {
    if (e.pointerType === "mouse" && e.button !== 0) return
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
    const pointerId = e.pointerId
    e.currentTarget.setPointerCapture(pointerId)

    function onMove(moveEvent: PointerEvent) {
      if (moveEvent.pointerId !== pointerId) return
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

    function onUp(upEvent: PointerEvent) {
      if (upEvent.pointerId !== pointerId) return
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
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

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
  }

  /** Ctrl-drag copy: tracks the copy's live position in `copyDragPreview`, separate from `rectEdit`. */
  function handleRectCopyDragPointerDown(e: React.PointerEvent, stamp: StampJson, rect: StampRect) {
    const draggedRef = { current: false }
    setCopyDragPreview({ stampId: stamp.id, rect })
    // Capture the start position at mousedown — a lazily-captured start gives a 0,0 first delta.
    const startClientX = e.clientX
    const startClientY = e.clientY
    const pointerId = e.pointerId

    function onMove(moveEvent: PointerEvent) {
      if (moveEvent.pointerId !== pointerId) return
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

    function onUp(upEvent: PointerEvent) {
      if (upEvent.pointerId !== pointerId) return
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
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
            // Edit mode moves onto the new copy immediately; rectEdit is seeded to avoid a flash.
            onSuccess: (data) => {
              setCopyDragPreview(null)
              setSelectedStampId(data.stamp_id)
              setRectEdit({ stampId: data.stamp_id, rect: copyRect, handle: null })
            },
          },
        )
        return current
      })
    }

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
  }

  /** Dragging the icon snaps it to the nearest of the 8 anchor points, by pixel distance. */
  function handleIconAnchorDragPointerDown(e: React.PointerEvent, stampId: string, rect: StampRect) {
    if (e.pointerType === "mouse" && e.button !== 0) return
    e.stopPropagation()
    rectEditedRef.current = false
    setRectEdit({ stampId, rect, handle: null })
    setActiveRectEditStampId(stampId)
    const pointerId = e.pointerId
    e.currentTarget.setPointerCapture(pointerId)

    function onMove(moveEvent: PointerEvent) {
      if (moveEvent.pointerId !== pointerId) return
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
        // Compare in real pixels — percent-space distances bias on non-square rects.
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

    function onUp(upEvent: PointerEvent) {
      if (upEvent.pointerId !== pointerId) return
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
      setActiveRectEditStampId(null)
      if (rectEditedRef.current) {
        setRectEdit((current) => {
          if (current) commitRectEdit(current.stampId, current.rect)
          return current
        })
      }
    }

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
  }

  /** Min rect size (%) — keeps a resize from collapsing the rect to nothing. */
  const MIN_RECT_SIZE = 2

  /** Corner handles adjust both axes; edge midpoints one. Shift locks the rect's pixel aspect
   * ratio (the percent fields aren't one physical scale on a non-square image). */
  function handleResizeHandlePointerDown(e: React.PointerEvent, stampId: string, rect: StampRect, handle: StampAnchor) {
    if (e.pointerType === "mouse" && e.button !== 0) return
    e.stopPropagation()
    rectEditedRef.current = false
    setRectEdit({ stampId, rect, handle })
    setActiveRectEditStampId(stampId)
    const startClientX = e.clientX
    const startClientY = e.clientY
    const pointerId = e.pointerId
    e.currentTarget.setPointerCapture(pointerId)
    const right = rect.x + rect.width
    const bottom = rect.y + rect.height
    const affectsLeft = handle === "tl" || handle === "l" || handle === "bl"
    const affectsRight = handle === "tr" || handle === "r" || handle === "br"
    const affectsTop = handle === "tl" || handle === "t" || handle === "tr"
    const affectsBottom = handle === "bl" || handle === "b" || handle === "br"

    function onMove(moveEvent: PointerEvent) {
      if (moveEvent.pointerId !== pointerId) return
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
        const pixelRatio = (rect.width * imgRect.width) / (rect.height * imgRect.height)
        const onlyHorizontal = affectsLeft || affectsRight
        const onlyVertical = affectsTop || affectsBottom
        // Corner handles follow the axis the cursor moved further along; an edge handle derives
        // its fixed axis from the dragged one.
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
        x = affectsLeft ? right - width : rect.x
        y = affectsTop ? bottom - height : rect.y
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

    function onUp(upEvent: PointerEvent) {
      if (upEvent.pointerId !== pointerId) return
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", onUp)
      setActiveRectEditStampId(null)
      if (rectEditedRef.current) {
        setRectEdit((current) => {
          if (current) commitRectEdit(current.stampId, current.rect)
          return current
        })
      }
    }

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", onUp)
  }

  useEffect(() => {
    if (!selectedStampId || !loggedIn) return
    function onKeyDown(e: KeyboardEvent) {
      const active = document.activeElement
      if (active instanceof HTMLInputElement || active instanceof HTMLTextAreaElement) return
      if (!selectedStampId) return
      const stamp = stamps.data?.result.find((s) => s.id === selectedStampId)
      const storedRect = stamp ? parseStampRect(stamp.rect) : null
      const rect = rectEdit?.stampId === selectedStampId ? rectEdit.rect : storedRect
      switch (e.key) {
        case "t":
        case "T":
          if (!stamp || !rect) return
          e.preventDefault()
          // stopImmediatePropagation (not stopPropagation) suppresses Reader.tsx's sibling window
          // listener (b/Backspace collision); every branch takes exclusive ownership of its key.
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
          const stepXPercent = (1 / imgRect.width) * 100
          const stepYPercent = (1 / imgRect.height) * 100
          const stampId = stamp.id
          setRectEdit((current) => {
            const base = current?.stampId === stampId ? current.rect : rect
            let { x, y } = base
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
    window.addEventListener("keydown", onKeyDown, true)
    return () => window.removeEventListener("keydown", onKeyDown, true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedStampId, stamps.data, rectEdit, loggedIn])

  useEffect(() => {
    return () => {
      if (arrowNudgeCommitTimeout.current) clearTimeout(arrowNudgeCommitTimeout.current)
    }
  }, [])

  if (!visible) return null

  return (
    <>
      {selectedStampId && (
        <div
          style={{ position: "fixed", inset: 0, zIndex: 20, cursor: "default" }}
          onPointerDown={(e) => e.preventDefault()}
          onClick={() => setSelectedStampId(null)}
        />
      )}
      <div
        ref={wrapperRef}
        style={{
          position: "absolute",
          left: imgBounds?.left ?? 0,
          top: imgBounds?.top ?? 0,
          width: imgBounds?.width ?? 0,
          height: imgBounds?.height ?? 0,
          pointerEvents: "none",
          zIndex: 23,
        }}
      >
        {imgBounds &&
          [...(stamps.data?.result ?? [])]
            .sort((a, b) => (parseStampRect(a.rect)?.layer ?? 0) - (parseStampRect(b.rect)?.layer ?? 0))
            .map((stamp) => {
            const [xStr, yStr] = stamp.position.split(",")
            const stored = { x: Number(xStr), y: Number(yStr) }
            if (Number.isNaN(stored.x) || Number.isNaN(stored.y)) return null
            const isDragging = activeDragStampId === stamp.id

            const storedRect = parseStampRect(stamp.rect)
            const rect = rectEdit?.stampId === stamp.id ? rectEdit.rect : storedRect
            const isSelected = selectedStampId === stamp.id
            const isHovered = hoveredStampId === stamp.id
            const isRectEditing = activeRectEditStampId === stamp.id

            const iconPos = rect ? anchorOnRect(rect) : (drag?.stampId === stamp.id ? drag : stored)

            return (
              <div key={stamp.id} data-stamp-id={stamp.id}>
                {rect && (rect.display === "always" || isHovered || isSelected) && (
                  <div
                    onPointerDown={(e) => {
                      rectEditedRef.current = false
                      if (!isSelected || !loggedIn) return
                      handleRectMovePointerDown(e, stamp, rect)
                    }}
                    onClick={(e) => {
                      if (rectEditedRef.current || !loggedIn) {
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
                      touchAction: "none",
                      ...rectFillStyle(rect),
                    }}
                  >
                    {isSelected &&
                      RESIZE_HANDLES.map((h) => {
                        const p = anchorPercent(h)
                        return (
                          <div
                            key={h}
                            onPointerDown={(e) => loggedIn && handleResizeHandlePointerDown(e, stamp.id, rect, h)}
                            style={{
                              position: "absolute",
                              left: `${p.x}%`,
                              top: `${p.y}%`,
                              transform: "translate(-50%, -50%)",
                              touchAction: "none",
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
                <Tooltip label={stamp.content} wrapperStyle={{ position: "static" }} anchor="cursor">
                  <div
                    className="marker"
                    style={{
                      left: `${iconPos.x}%`,
                      top: `${iconPos.y}%`,
                      cursor: rect ? (isSelected ? "grab" : "pointer") : isDragging ? "grabbing" : "grab",
                      pointerEvents: "auto",
                      touchAction: "none",
                      ...(stamp.icon && { backgroundImage: "none" }),
                    }}
                    onMouseEnter={() => rect && setHoveredStampId(stamp.id)}
                    onMouseLeave={() => !isRectEditing && setHoveredStampId(null)}
                    onPointerDown={(e) => {
                      if (!loggedIn) return
                      if (rect) {
                        rectEditedRef.current = false
                        if (isSelected) handleIconAnchorDragPointerDown(e, stamp.id, rect)
                        return
                      }
                      draggedRef.current = false
                      handleMarkerPointerDown(e, stamp.id, iconPos.x, iconPos.y)
                    }}
                    onClick={(e) => {
                      if (draggedRef.current || rectEditedRef.current || !loggedIn) {
                        e.preventDefault()
                        return
                      }
                      setSelectedStampId(stamp.id)
                    }}
                    onDoubleClick={() => {
                      if (!loggedIn) return
                      void openEditorForExisting(stamp.id)
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault()
                      if (!loggedIn) return
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
                {!supportsHover && (
                  <StaticTooltip
                    xPercent={iconPos.x}
                    yPercent={iconPos.y}
                    {...stampTooltipPlacement(rect?.anchor ?? null)}
                    label={stamp.content}
                  />
                )}
              </div>
            )
          })}

          {placementDrag && (
            <div
              style={{
                position: "absolute",
                left: `${Math.min(placementDrag.startX, placementDrag.curX)}%`,
                top: `${Math.min(placementDrag.startY, placementDrag.curY)}%`,
                width: `${Math.abs(placementDrag.curX - placementDrag.startX)}%`,
                height: `${Math.abs(placementDrag.curY - placementDrag.startY)}%`,
                border: `2px dashed ${lastPickedRectStyle().color}`,
                background: `${lastPickedRectStyle().color}33`,
                boxSizing: "border-box",
                pointerEvents: "none",
                zIndex: Z_OVERLAY_CONTENT,
              }}
            />
          )}

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

/** Anchor point on the rect's border, converted to page-relative percent. */
function anchorOnRect(rect: StampRect): { x: number; y: number } {
  const p = anchorPercent(rect.anchor)
  return {
    x: rect.x + (rect.width * p.x) / 100,
    y: rect.y + (rect.height * p.y) / 100,
  }
}

/** Tooltip side: extend the rect-center→icon vector outward so the tooltip never overlaps the rect. */
function stampTooltipPlacement(anchor: StampAnchor | null): { side: TooltipSide; align: TooltipAlign } {
  switch (anchor) {
    case "tl":
      return { side: "top", align: "start" }
    case "t":
      return { side: "top", align: "center" }
    case "tr":
      return { side: "top", align: "end" }
    case "r":
      return { side: "right", align: "center" }
    case "br":
      return { side: "bottom", align: "end" }
    case "b":
      return { side: "bottom", align: "center" }
    case "bl":
      return { side: "bottom", align: "start" }
    case "l":
      return { side: "left", align: "center" }
    case null:
      return { side: "bottom", align: "center" }
  }
}

/** Standard 8-way resize cursor for each handle. */
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

/** Rect interior style per `StampFill`: solid/stripes are translucent overlays; mosaic/blur
 * obscure via backdrop-filter (no real CSS pixelation — mosaic is a stronger blur). */
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
