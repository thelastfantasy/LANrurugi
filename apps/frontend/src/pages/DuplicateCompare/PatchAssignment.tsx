import { Feedback, PointerActivationConstraints, PointerSensor } from "@dnd-kit/dom"
import { move } from "@dnd-kit/helpers"
import { DragDropProvider, DragOverlay } from "@dnd-kit/react"
import { useSortable } from "@dnd-kit/react/sortable"
import { useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useCreateDuplicatePatch } from "@/api/hooks"
import type { ComparisonSide, DuplicatePatchPick, DuplicatePatchResult } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { useIsNarrowViewport } from "@/hooks"
import { ScrollRow, THUMB_ASPECT_RATIO } from "@/pages/Upload/shared"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

import { pageRanges } from "./PageAlignmentBand"
import { SideBadge } from "./SideBadge"
import { Slot } from "./Slot"

const PLACED_BORDER = "2px dashed rgba(255,152,0,0.9)"
const SELECTED_OUTLINE = "3px solid #fff"

function thumb(archiveId: string, page: number) {
  return `/api/archives/${encodeURIComponent(archiveId)}/thumbnail?page=${page + 1}`
}
const sourceId = (page: number) => `source-${page}`
const targetId = (page: number) => `target-${page}`
const isSource = (id: string) => id.startsWith("source-")
const pageOf = (id: string) => Number(id.replace(/^(source|target)-/, ""))

export interface PatchSide {
  side: ComparisonSide
  id: string
  /** Patch-merged page count. */
  pages: number
  /** Pages already coming from this side's own patch. */
  patchPages: number[]
}

/**
 * Places the pages only `source` has into `target` (spec 024): drag them between the target's
 * pages, or tap one and then tap the target page it should follow — dragging inside a scrolling
 * strip fights the scroll on a phone. Each page starts at the position the alignment suggests.
 */
export function PatchAssignment({
  source,
  target,
  unmatched,
  onDone,
  onCancel,
}: {
  source: PatchSide
  target: PatchSide
  unmatched: { page_index: number; default_insert_after: number | null }[]
  onDone: (result: DuplicatePatchResult) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const narrow = useIsNarrowViewport()
  const create = useCreateDuplicatePatch()
  const sourceRowRef = useRef<HTMLDivElement>(null)
  const targetRowRef = useRef<HTMLDivElement>(null)
  const unmatchedSet = useMemo(() => new Set(unmatched.map((p) => p.page_index)), [unmatched])
  const targetPatch = useMemo(() => new Set(target.patchPages), [target.patchPages])

  function suggested(): Record<string, string[]> {
    const pool: string[] = []
    const row = Array.from({ length: target.pages }, (_, i) => targetId(i))
    const sorted = [...unmatched].sort((x, y) => x.page_index - y.page_index)
    let lastAfter = -1
    let offset = 0
    let front = 0
    for (const p of sorted) {
      if (p.default_insert_after == null) {
        // No matched page precedes it: before the target's first page, in source order.
        row.splice(front, 0, sourceId(p.page_index))
        front++
        continue
      }
      if (p.default_insert_after !== lastAfter) {
        lastAfter = p.default_insert_after
        offset = 0
      }
      const at = row.indexOf(targetId(p.default_insert_after))
      if (at < 0) {
        pool.push(sourceId(p.page_index))
        continue
      }
      row.splice(at + 1 + offset, 0, sourceId(p.page_index))
      offset++
    }
    return { source: pool, target: row }
  }

  const [groups, setGroups] = useState<Record<string, string[]>>(suggested)
  const [selected, setSelected] = useState<string | null>(null)
  const placed = (groups.target ?? []).filter(isSource)

  /** Moves `id` (a source page, wherever it is) to right after `afterId`, or to the front. */
  function placeSelected(afterId: string | null) {
    if (!selected) return
    setGroups((g) => {
      const sourcePool = (g.source ?? []).filter((x) => x !== selected)
      const row = (g.target ?? []).filter((x) => x !== selected)
      const at = afterId === null ? 0 : row.indexOf(afterId) + 1
      row.splice(at, 0, selected)
      return { source: sourcePool, target: row }
    })
    setSelected(null)
  }

  function unplaceSelected() {
    if (!selected) return
    setGroups((g) => ({
      source: [...(g.source ?? []).filter((x) => x !== selected), selected].sort((x, y) => pageOf(x) - pageOf(y)),
      target: (g.target ?? []).filter((x) => x !== selected),
    }))
    setSelected(null)
  }

  /** Each placed page lands before the next target page after it (or past the end). */
  function picks(): DuplicatePatchPick[] {
    const out: DuplicatePatchPick[] = []
    let targetSeen = 0
    for (const id of groups.target ?? []) {
      if (isSource(id)) out.push({ source_page: pageOf(id), insert_before: targetSeen })
      else targetSeen++
    }
    return out
  }

  /** "p. 67–69 → after B p. 66" lines for the summary. */
  const summary = useMemo(() => {
    const lines: { pages: number[]; after: number | null }[] = []
    let lastTarget: number | null = null
    let run: number[] = []
    for (const id of groups.target ?? []) {
      if (isSource(id)) run.push(pageOf(id))
      else {
        if (run.length) lines.push({ pages: run, after: lastTarget })
        run = []
        lastTarget = pageOf(id)
      }
    }
    if (run.length) lines.push({ pages: run, after: lastTarget })
    return lines
  }, [groups.target])

  async function submit() {
    try {
      const response = await create.mutateAsync({ target: target.id, source: source.id, pages: picks() })
      onDone(response.result)
    } catch {
      // The mutation's own error is shown below the buttons.
    }
  }

  const stripHeight = narrow ? 150 : 210

  return (
    <div
      role="dialog"
      aria-modal="true"
      style={{
        position: "fixed",
        inset: 0,
        height: "100dvh",
        zIndex: 9600,
        background: "#0e0e10",
        color: "#f2f2f2",
        display: "flex",
        flexDirection: "column",
        gap: 10,
        boxSizing: "border-box",
        padding: narrow
          ? "max(8px, env(safe-area-inset-top)) max(8px, env(safe-area-inset-right)) max(8px, env(safe-area-inset-bottom)) max(8px, env(safe-area-inset-left))"
          : 16,
        textAlign: "left",
      }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <h3 style={{ margin: 0, flex: 1, minWidth: 0, fontSize: "1rem", color: "inherit", display: "flex", alignItems: "center", gap: 6 }}>
          <i className="fa fa-puzzle-piece" aria-hidden="true" />
          {t("duplicates.patchTitle")}
          <SideBadge side={source.side} />
          <i className="fa fa-long-arrow-alt-right" aria-hidden="true" />
          <SideBadge side={target.side} />
        </h3>
        <IconButton icon="fa fa-times" size={36} onClick={onCancel} title={t("common.cancel")} className="modal-close-btn" style={{ borderRadius: "50%", touchAction: "manipulation" }} />
      </div>
      <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8 }}>
        {t(narrow ? "duplicates.patchHintTouch" : "duplicates.patchHint", {
          source: source.side.toUpperCase(),
          target: target.side.toUpperCase(),
          placed: placed.length,
          total: unmatched.length,
        })}
      </div>

      <DragDropProvider
        sensors={[PointerSensor.configure({ activationConstraints: [new PointerActivationConstraints.Distance({ value: 6 })] })]}
        onDragOver={(event) => {
          if (event.operation.source?.type !== "column") setGroups((g) => move(g, event))
        }}
      >
        <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: 12, overflowY: "auto" }}>
          <section style={{ display: "flex", flexDirection: "column", height: stripHeight + 28 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: FONT_SIZE_XS, opacity: 0.8, marginBottom: 4 }}>
              <SideBadge side={source.side} size={16} />
              <span style={{ flex: 1 }}>{t("duplicates.patchSourceRow")}</span>
              <button type="button" className="stdbtn" style={{ margin: 0 }} onClick={() => { setGroups(suggested()); setSelected(null) }}>
                <i className="fa fa-magic" aria-hidden="true" style={{ marginRight: 4 }} />
                {t("duplicates.patchSuggested")}
              </button>
            </div>
            <Slot id="source">
              <ScrollRow
                count={source.pages}
                rowRef={sourceRowRef}
                renderPage={(page) => {
                  if (!unmatchedSet.has(page)) {
                    return <StaticPage key={page} src={thumb(source.id, page)} label={page + 1} dim />
                  }
                  const id = sourceId(page)
                  const pool = groups.source ?? []
                  if (pool.includes(id)) {
                    return (
                      <SortablePage
                        key={id}
                        id={id}
                        group="source"
                        index={pool.indexOf(id)}
                        src={thumb(source.id, page)}
                        label={page + 1}
                        selected={selected === id}
                        onTap={() => setSelected(selected === id ? null : id)}
                      />
                    )
                  }
                  // Placed already: a ghost keeps the source strip's page order readable.
                  return <StaticPage key={page} src={thumb(source.id, page)} label={page + 1} ghost />
                }}
              />
            </Slot>
          </section>

          <section style={{ display: "flex", flexDirection: "column", height: stripHeight + 28 }}>
            <div style={{ display: "flex", alignItems: "center", gap: 8, fontSize: FONT_SIZE_XS, opacity: 0.8, marginBottom: 4 }}>
              <SideBadge side={target.side} size={16} />
              <span style={{ flex: 1 }}>{t("duplicates.patchTargetRow")}</span>
              {selected && (
                <>
                  <button type="button" className="stdbtn" style={{ margin: 0 }} onClick={() => placeSelected(null)}>
                    {t("duplicates.patchToFront")}
                  </button>
                  {(groups.target ?? []).includes(selected) && (
                    <button type="button" className="stdbtn" style={{ margin: 0 }} onClick={unplaceSelected}>
                      {t("duplicates.patchUnplace")}
                    </button>
                  )}
                </>
              )}
            </div>
            <Slot id="target">
              <ScrollRow
                count={1}
                rowRef={targetRowRef}
                renderPage={() => (
                  <div key="row" style={{ display: "flex", gap: 6, height: "100%" }}>
                    {(groups.target ?? []).map((id, i) =>
                      isSource(id) ? (
                        <SortablePage
                          key={id}
                          id={id}
                          group="target"
                          index={i}
                          src={thumb(source.id, pageOf(id))}
                          label={pageOf(id) + 1}
                          side={source.side}
                          placed
                          selected={selected === id}
                          onTap={() => setSelected(selected === id ? null : id)}
                        />
                      ) : (
                        <SortablePage
                          key={id}
                          id={id}
                          group="target"
                          index={i}
                          src={thumb(target.id, pageOf(id))}
                          label={pageOf(id) + 1}
                          side={target.side}
                          fixed
                          fromPatch={targetPatch.has(pageOf(id))}
                          dropHint={selected !== null}
                          onTap={() => placeSelected(id)}
                        />
                      ),
                    )}
                  </div>
                )}
              />
            </Slot>
          </section>

          <section style={{ fontSize: FONT_SIZE_SM, background: "rgba(255,255,255,0.05)", borderRadius: 6, padding: "8px 10px" }}>
            {summary.length === 0 ? (
              <span style={{ opacity: 0.7 }}>{t("duplicates.patchNothingPlaced")}</span>
            ) : (
              summary.map((line) => (
                <div key={line.pages[0]}>
                  {t("duplicates.patchLine", {
                    source: source.side.toUpperCase(),
                    pages: pageRanges(line.pages),
                    where:
                      line.after === null
                        ? t("duplicates.patchAtFront", { target: target.side.toUpperCase() })
                        : t("duplicates.patchAfter", { target: target.side.toUpperCase(), page: line.after + 1 }),
                  })}
                </div>
              ))
            )}
          </section>
        </div>

        <DragOverlay>
          {(dragged) => (
            <div style={{ width: 120, aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden", boxShadow: "0 4px 12px rgba(0,0,0,0.6)" }}>
              <img src={thumb(source.id, pageOf(String(dragged.id)))} alt="" style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
            </div>
          )}
        </DragOverlay>
      </DragDropProvider>

      {create.isError && (
        <div style={{ fontSize: FONT_SIZE_SM, color: "#ff8a80" }}>{String(create.error)}</div>
      )}
      <div style={{ display: "grid", gridTemplateColumns: "1fr 1fr", gap: 8, maxWidth: narrow ? undefined : 420, alignSelf: narrow ? "stretch" : "flex-end", width: narrow ? "100%" : undefined }}>
        <button type="button" className="stdbtn" style={{ margin: 0, minHeight: narrow ? 44 : 34 }} onClick={onCancel}>
          {t("common.cancel")}
        </button>
        <button
          type="button"
          className="stdbtn"
          style={{ margin: 0, minHeight: narrow ? 44 : 34, fontWeight: 700 }}
          disabled={placed.length === 0 || create.isPending}
          onClick={() => void submit()}
        >
          {create.isPending && <i className="fa fa-spinner fa-spin" aria-hidden="true" style={{ marginRight: 6 }} />}
          {t("duplicates.patchApply", { count: placed.length, target: target.side.toUpperCase() })}
        </button>
      </div>
    </div>
  )
}

function PageLabel({ label, fromPatch }: { label: number; fromPatch?: boolean }) {
  return (
    <span
      style={{
        position: "absolute",
        left: 4,
        bottom: 4,
        padding: "0 4px",
        borderRadius: 3,
        fontSize: FONT_SIZE_XS,
        background: "rgba(0,0,0,0.7)",
        color: "#fff",
        pointerEvents: "none",
      }}
    >
      {label}
      {fromPatch && <i className="fa fa-puzzle-piece" aria-hidden="true" style={{ marginLeft: 3 }} />}
    </span>
  )
}

function StaticPage({ src, label, dim, ghost }: { src: string; label: number; dim?: boolean; ghost?: boolean }) {
  return (
    <div style={{ position: "relative", flexShrink: 0, height: "100%", aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden", opacity: ghost ? 0.3 : dim ? 0.55 : 1 }}>
      <img src={src} alt="" loading="lazy" draggable={false} style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
      <PageLabel label={label} />
    </div>
  )
}

/** A page in either strip: a source page (draggable, tappable) or a fixed target page (a drop
 *  neighbour that only takes part in ordering, and a tap target for placing the selected page). */
function SortablePage({
  id,
  group,
  index,
  src,
  label,
  side,
  placed,
  fixed,
  fromPatch,
  selected,
  dropHint,
  onTap,
}: {
  id: string
  group: string
  index: number
  src: string
  label: number
  side?: ComparisonSide
  placed?: boolean
  fixed?: boolean
  fromPatch?: boolean
  selected?: boolean
  dropHint?: boolean
  onTap: () => void
}) {
  const { ref, isDragging } = useSortable({
    id,
    group,
    index,
    type: "item",
    accept: ["item"],
    disabled: fixed ? { draggable: true } : false,
    plugins: [Feedback.configure({ feedback: "clone" })],
  })
  return (
    <div
      ref={ref}
      onClick={onTap}
      role="button"
      aria-pressed={selected}
      title={side ? `${side.toUpperCase()} ${label}` : String(label)}
      style={{
        position: "relative",
        flexShrink: 0,
        height: "100%",
        aspectRatio: THUMB_ASPECT_RATIO,
        boxSizing: "border-box",
        borderRadius: 4,
        overflow: "hidden",
        cursor: fixed ? (dropHint ? "copy" : "default") : "grab",
        // Fixed pages keep native horizontal panning so the strip still scrolls under a finger.
        touchAction: fixed ? "pan-x" : "none",
        border: fixed ? (dropHint ? "2px solid rgba(255,255,255,0.35)" : "2px solid transparent") : PLACED_BORDER,
        outline: selected ? SELECTED_OUTLINE : undefined,
        outlineOffset: -3,
        visibility: isDragging ? "hidden" : "visible",
        background: placed ? "rgba(255,152,0,0.15)" : undefined,
      }}
    >
      <img src={src} alt="" loading="lazy" draggable={false} style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
      <PageLabel label={label} fromPatch={fromPatch} />
    </div>
  )
}
