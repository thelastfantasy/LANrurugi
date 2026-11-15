import { useState } from "react"
import { useTranslation } from "react-i18next"
import {
  FaCheckDouble,
  FaCompress,
  FaEject,
  FaGripVertical,
  FaHammer,
  FaList,
  FaSquareCheck,
  FaTableCellsLarge,
  FaWindowMinimize,
  FaXmark,
} from "react-icons/fa6"

import { useArchiveMetadata } from "@/api/hooks"
import { type DragHandleProps, Drawer, DrawerFooter, SortableList } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { SELECTION_VIEW_KEY } from "@/lib/storageKeys"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

type SelectionView = "list" | "grid"

function readView(): SelectionView {
  try {
    return localStorage.getItem(SELECTION_VIEW_KEY) === "grid" ? "grid" : "list"
  } catch {
    return "list"
  }
}

/** The selection mode's panel: what is selected (drag to set the merged Tankoubon's volume order)
 * and what can be done with it. Swiping it away, Escape or the backdrop only minimize it to
 * `SelectionFab`, so the grid stays reachable for picking more; its ✕ leaves selection mode. */
export function SelectionDrawer({
  open,
  selectedIds,
  filteredCount,
  canMerge,
  onMinimize,
  onExit,
  onReorder,
  onRemove,
  onSelectPage,
  onClear,
  onRunBatch,
  onRunBatchOnFilter,
  onMerge,
}: {
  open: boolean
  selectedIds: string[]
  filteredCount: number
  canMerge: boolean
  onMinimize: () => void
  onExit: () => void
  onReorder: (ids: string[]) => void
  onRemove: (id: string) => void
  onSelectPage: () => void
  onClear: () => void
  onRunBatch: () => void
  onRunBatchOnFilter: () => void
  onMerge: () => void
}) {
  const { t } = useTranslation()
  const count = selectedIds.length
  const [view, setView] = useState<SelectionView>(readView)
  const toggleView = () => {
    const next = view === "list" ? "grid" : "list"
    setView(next)
    try {
      localStorage.setItem(SELECTION_VIEW_KEY, next)
    } catch {
      // A per-viewer convenience; without storage it just resets next time.
    }
  }
  const viewLabel = t(view === "list" ? "library.selectionViewGrid" : "library.selectionViewList") ?? undefined
  return (
    <Drawer
      open={open}
      onClose={onMinimize}
      onCloseButton={onExit}
      width={420}
      title={
        <span className="inline-flex items-center gap-2">
          <FaSquareCheck aria-hidden="true" />
          {t("app.selection")}
          {count > 0 && (
            <span style={{ fontWeight: 400, opacity: 0.75 }}>{t("library.selected", { n: count })}</span>
          )}
        </span>
      }
      headerActions={
        <>
          {count > 0 && (
            <IconButton
              variant="ghost-btn"
              icon={view === "list" ? <FaTableCellsLarge size={16} /> : <FaList size={16} />}
              size={30}
              title={viewLabel}
              aria-label={viewLabel}
              onClick={toggleView}
            />
          )}
          <IconButton
            variant="ghost-btn"
            icon={<FaWindowMinimize size={16} />}
            size={30}
            title={t("library.minimizeSelection") ?? undefined}
            aria-label={t("library.minimizeSelection") ?? undefined}
            onClick={onMinimize}
          />
        </>
      }
    >
      {count === 0 ? (
        <div
          className="flex flex-col items-center justify-center gap-3 text-center"
          style={{ padding: "48px 16px", opacity: 0.8 }}
        >
          <i className="fa fa-glasses fa-3x" aria-hidden="true"></i>
          <span>{t("library.clickArchivesToAddThem")}</span>
          <span style={{ fontSize: 12 }}>{t("Batch operations apply to the current filter")}</span>
        </div>
      ) : (
        <SortableList
          items={selectedIds}
          getId={(id) => id}
          onReorder={onReorder}
          direction={view === "grid" ? "grid" : "vertical"}
          gridMinItemWidth={140}
          renderItem={(id, handle) =>
            view === "grid" ? (
              <SelectedTile id={id} handle={handle} onRemove={onRemove} />
            ) : (
              <SelectedRow id={id} handle={handle} onRemove={onRemove} />
            )
          }
        />
      )}
      <DrawerFooter>
        <div className="grid grid-cols-2 gap-2">
          <button type="button" className="stdbtn" style={FOOTER_BUTTON} onClick={onSelectPage}>
            <FaCheckDouble aria-hidden="true" />
            {t("Select All on Page")}
          </button>
          <button type="button" className="stdbtn" style={FOOTER_BUTTON} disabled={count === 0} onClick={onClear}>
            <FaEject aria-hidden="true" />
            {t("Clear Selection")}
          </button>
          {/* With nothing selected this runs over everything the filter matches — "tag-scan every
              untagged archive" should not need hundreds of ticks. */}
          <button
            type="button"
            className="stdbtn"
            style={{ ...FOOTER_BUTTON, gridColumn: canMerge ? undefined : "1 / -1" }}
            onClick={count === 0 ? onRunBatchOnFilter : onRunBatch}
          >
            <FaHammer aria-hidden="true" />
            {count === 0 ? t("Batch process all {{n}} results", { n: filteredCount }) : t("Batch Operations")}
          </button>
          {canMerge && (
            <button type="button" className="stdbtn" style={FOOTER_BUTTON} onClick={onMerge}>
              <FaCompress aria-hidden="true" />
              {t("Merge into Tankoubon")}
            </button>
          )}
        </div>
      </DrawerFooter>
    </Drawer>
  )
}

// Legacy `.stdbtn` carries its own margins and minimum width; in the grid each fills its cell.
const FOOTER_BUTTON = {
  margin: 0,
  width: "100%",
  minWidth: 0,
  height: 36,
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  gap: 6,
} as const

const thumbSrc = (id: string) =>
  isTankoubonId(id) ? `/api/tankoubons/${id}/thumbnail` : `/api/archives/${id}/thumbnail`

function SelectedRow({
  id,
  handle,
  onRemove,
}: {
  id: string
  handle: DragHandleProps
  onRemove: (id: string) => void
}) {
  const { t } = useTranslation()
  const metadata = useArchiveMetadata(id)
  return (
    <div
      className="flex items-center gap-2"
      style={{
        padding: "6px 0",
        borderBottom: "1px solid rgba(128,128,128,0.25)",
        opacity: handle.isDragging ? 0.85 : undefined,
      }}
    >
      {/* Only the grip starts a drag, so the list still scrolls under a finger; `touch-action`
          and the swipe-ignore keep the browser and the drawer's swipe out of that drag. */}
      <span
        {...handle.attributes}
        {...handle.listeners}
        data-base-ui-swipe-ignore=""
        aria-label={t("library.dragToReorder") ?? undefined}
        className="flex shrink-0 items-center justify-center"
        style={{
          width: 28,
          height: 56,
          cursor: handle.isDragging ? "grabbing" : "grab",
          touchAction: "none",
          opacity: 0.6,
        }}
      >
        <FaGripVertical aria-hidden="true" />
      </span>
      <img
        src={thumbSrc(id)}
        alt=""
        className="shrink-0 rounded object-cover"
        style={{ width: 40, height: 56 }}
      />
      <span className="line-clamp-2 min-w-0 flex-1 break-words text-left" style={{ fontSize: 13 }}>
        {metadata.data?.title ?? ""}
      </span>
      <IconButton
        variant="ghost-btn"
        icon={<FaXmark size={16} />}
        size={32}
        title={t("library.removeFromSelection") ?? undefined}
        aria-label={t("library.removeFromSelection") ?? undefined}
        onClick={() => onRemove(id)}
      />
    </div>
  )
}

/** Grid view: the cover at the column's width, title under it. The grip and the remove button sit
 * on the cover as round `.modal-close-btn` chips, which every theme already colors for that. */
function SelectedTile({
  id,
  handle,
  onRemove,
}: {
  id: string
  handle: DragHandleProps
  onRemove: (id: string) => void
}) {
  const { t } = useTranslation()
  const metadata = useArchiveMetadata(id)
  return (
    <div className="flex flex-col gap-1" style={{ opacity: handle.isDragging ? 0.85 : undefined }}>
      <div className="relative">
        <img
          src={thumbSrc(id)}
          alt=""
          className="block w-full rounded object-cover"
          style={{ aspectRatio: "5 / 7" }}
        />
        <span
          {...handle.attributes}
          {...handle.listeners}
          data-base-ui-swipe-ignore=""
          aria-label={t("library.dragToReorder") ?? undefined}
          className="modal-close-btn absolute flex items-center justify-center rounded-full"
          style={{
            ...TILE_CHIP,
            left: 4,
            top: 4,
            cursor: handle.isDragging ? "grabbing" : "grab",
            touchAction: "none",
          }}
        >
          <FaGripVertical aria-hidden="true" />
        </span>
        <button
          type="button"
          title={t("library.removeFromSelection") ?? undefined}
          aria-label={t("library.removeFromSelection") ?? undefined}
          className="modal-close-btn absolute flex items-center justify-center rounded-full"
          style={{ ...TILE_CHIP, right: 4, top: 4, padding: 0, cursor: "pointer" }}
          onClick={() => onRemove(id)}
        >
          <FaXmark aria-hidden="true" />
        </button>
      </div>
      <span className="line-clamp-2 break-words text-left" style={{ fontSize: 12, lineHeight: 1.35 }}>
        {metadata.data?.title ?? ""}
      </span>
    </div>
  )
}

const TILE_CHIP = { width: 30, height: 30, fontSize: 14, margin: 0 } as const

/** Selection mode's always-there way back to `SelectionDrawer` while it is minimized. */
export function SelectionFab({ count, onOpen }: { count: number; onOpen: () => void }) {
  const { t } = useTranslation()
  return (
    <button
      type="button"
      className="stdbtn"
      aria-label={t("library.openSelection") ?? undefined}
      onClick={onOpen}
      style={{
        position: "fixed",
        right: 16,
        bottom: "calc(16px + env(safe-area-inset-bottom))",
        zIndex: 900,
        margin: 0,
        minWidth: 0,
        height: 48,
        padding: "0 18px",
        borderRadius: 24,
        display: "inline-flex",
        alignItems: "center",
        gap: 8,
        fontSize: 16,
        fontWeight: 600,
        boxShadow: "0 3px 10px rgba(0,0,0,0.35)",
      }}
    >
      <FaSquareCheck size={18} aria-hidden="true" />
      <span style={{ fontVariantNumeric: "tabular-nums" }}>{count}</span>
    </button>
  )
}
