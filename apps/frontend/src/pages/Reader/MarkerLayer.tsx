import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useAddStamp, useDeleteStamp, useStampsForPage, useUpdateStamp } from '../../api/hooks'
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../../theme'

// Mirrors legacy's stamp/marker feature (`~/LANraragi/public/js/reader.js`'s `addStamp`/
// `renderMarkers`/`loadStamps` + `.marker` in `lrr.css`): click-to-place a pin at a %-based
// position with a text label, drag to reposition, right-click to rename/delete. `imageRef` is the
// currently-visible page `<img>` a click's percentage position is measured against; `page` is
// that image's 1-indexed page number (the "left" slot in a double-page spread).

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

export default function MarkerLayer({
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
  const [menu, setMenu] = useState<ContextMenuState | null>(null)
  const [drag, setDrag] = useState<DragState | null>(null)
  const draggedRef = useRef(false)

  function handlePlacementClick(e: React.MouseEvent<HTMLDivElement>) {
    const img = imageRef.current
    if (!img) return
    const rect = img.getBoundingClientRect()
    const x = ((e.clientX - rect.left) / rect.width) * 100
    const y = ((e.clientY - rect.top) / rect.height) * 100
    onPlaced()
    const content = window.prompt(t('Enter Stamp name:') ?? undefined)
    if (content === null) return
    addStamp.mutate({ page, content, position: `${x.toFixed(2)},${y.toFixed(2)}` })
  }

  function handleMarkerPointerDown(e: React.MouseEvent<HTMLDivElement>, stampId: string, x: number, y: number) {
    if (e.button !== 0) return
    e.stopPropagation()
    draggedRef.current = false
    setDrag({ stampId, x, y })

    function onMove(moveEvent: MouseEvent) {
      const img = imageRef.current
      if (!img) return
      draggedRef.current = true
      const rect = img.getBoundingClientRect()
      const nx = clampPercent(((moveEvent.clientX - rect.left) / rect.width) * 100)
      const ny = clampPercent(((moveEvent.clientY - rect.top) / rect.height) * 100)
      setDrag({ stampId, x: nx, y: ny })
    }

    function onUp() {
      window.removeEventListener('mousemove', onMove)
      window.removeEventListener('mouseup', onUp)
      setDrag((current) => {
        if (current && draggedRef.current) {
          updateStamp.mutate({
            stampId: current.stampId,
            position: `${current.x.toFixed(2)},${current.y.toFixed(2)}`,
          })
        }
        return null
      })
    }

    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp)
  }

  if (!visible) return null

  return (
    <>
      {(stamps.data?.result ?? []).map((stamp) => {
        const [xStr, yStr] = stamp.position.split(',')
        const stored = { x: Number(xStr), y: Number(yStr) }
        if (Number.isNaN(stored.x) || Number.isNaN(stored.y)) return null
        const isDragging = drag?.stampId === stamp.id
        const pos = isDragging ? drag : stored
        return (
          <div
            key={stamp.id}
            className="marker"
            style={{ left: `${pos.x}%`, top: `${pos.y}%`, cursor: isDragging ? 'grabbing' : 'grab' }}
            title={stamp.content}
            onMouseDown={(e) => handleMarkerPointerDown(e, stamp.id, stored.x, stored.y)}
            onClick={(e) => {
              // A drag that actually moved the pin shouldn't also open the rename prompt via a
              // trailing click — mouseup after dragging still fires a click event.
              if (draggedRef.current) e.preventDefault()
            }}
            onContextMenu={(e) => {
              e.preventDefault()
              setMenu({ stampId: stamp.id, x: e.clientX, y: e.clientY })
            }}
          />
        )
      })}

      {/* Only present while marker-placement mode is armed (`S` key) — dims the page and captures
          the next click's %-position, matching legacy's `#overlay-page.focus-overlay` behavior,
          rather than always intercepting clicks meant for page-turning. */}
      {placementMode && (
        <div
          className="focus-overlay"
          style={{ display: 'block', cursor: 'cell', zIndex: 22 }}
          onClick={handlePlacementClick}
        />
      )}

      {menu && (
        <>
          <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }} onClick={() => setMenu(null)} />
          <div
            className="id1 marker-context-menu"
            style={{ position: 'fixed', top: menu.y, left: menu.x, zIndex: Z_OVERLAY_CONTENT, width: 180 }}
          >
            <ul style={{ listStyle: 'none', margin: 0, padding: '6px 0' }}>
              <li
                className="context-menu-item"
                style={{ padding: '4px 12px', cursor: 'pointer' }}
                onClick={() => {
                  const content = window.prompt(t('Enter Stamp name:') ?? undefined)
                  setMenu(null)
                  if (content !== null) updateStamp.mutate({ stampId: menu.stampId, content })
                }}
              >
                {t('Edit Marker')}
              </li>
              <li
                className="context-menu-item"
                style={{ padding: '4px 12px', cursor: 'pointer' }}
                onClick={() => {
                  setMenu(null)
                  deleteStamp.mutate(menu.stampId)
                }}
              >
                {t('Delete Marker')}
              </li>
            </ul>
          </div>
        </>
      )}
    </>
  )
}

function clampPercent(n: number): number {
  return Math.min(Math.max(n, 0), 100)
}
