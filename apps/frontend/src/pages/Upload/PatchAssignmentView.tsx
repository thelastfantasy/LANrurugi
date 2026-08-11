import { move } from "@dnd-kit/helpers"
import { DragDropProvider, DragOverlay } from "@dnd-kit/react"
import type Lenis from "lenis"
import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useComparePages } from "@/api/hooks"
import type { ExportPatchInsertion, UnmatchedPage } from "@/api/types"
import { IconButton } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { ScrollRow, THUMB_ASPECT_RATIO } from "./shared"
import { Slot } from "./Slot"
import { SourceDrag, SourceGhost, SourcePlain } from "./SourceItem"
import { TargetDrag, TargetThumbSortable } from "./TargetItem"

function pageId(i: number) { return `source-${i}` }
function targetId(i: number) { return `target-${i}` }
function isSourceItem(id: string) { return id.startsWith("source-") }

export function PatchAssignmentView({ queueItemId, sourceSide, targetSide, sourceTotalPages, unmatchedPages, onConfirm, onCancel }: {
  queueItemId: string; sourceSide: "a" | "b"; targetSide: "a" | "b"; sourceTotalPages: number
  unmatchedPages: UnmatchedPage[]; onConfirm: (i: ExportPatchInsertion[]) => void; onCancel: () => void
}) {
  const { t } = useTranslation()
  const sourceRowRef = useRef<HTMLDivElement>(null)
  const targetRowRef = useRef<HTMLDivElement>(null)
  const sourceLenisRef = useRef<{ lenis: Lenis; stepPx(): number } | null>(null)
  const targetLenisRef = useRef<{ lenis: Lenis; stepPx(): number } | null>(null)

  const targetPagesQuery = useComparePages(queueItemId, targetSide)
  const targetPages = targetPagesQuery.data?.pages ?? []
  const unmatchedIndices = useMemo(() => new Set(unmatchedPages.map((p) => p.page_index)), [unmatchedPages])
  const unmatchedByIndex = useMemo(() => new Map(unmatchedPages.map((p) => [p.page_index, p])), [unmatchedPages])

  function buildDefaultGroups(): Record<string, string[]> {
    const sourceItems: string[] = []
    const targetItems = targetPages.map((_, i) => targetId(i))
    for (const p of unmatchedPages) {
      const pid = pageId(p.page_index)
      if (p.default_insert_after == null) { sourceItems.push(pid); continue }
      // default_insert_after = target page index. Insert source page right after that target.
      const idx = targetItems.indexOf(targetId(p.default_insert_after))
      targetItems.splice(idx >= 0 ? idx + 1 : 0, 0, pid)
    }
    return { source: sourceItems, target: targetItems }
  }

  const [groups, setGroups] = useState<Record<string, string[]>>(() =>
    targetPages.length > 0 ? buildDefaultGroups() : { source: unmatchedPages.map((p) => pageId(p.page_index)), target: [] }
  )
  useEffect(() => { if (targetPages.length > 0) setGroups(buildDefaultGroups()) }, [targetPages]) // eslint-disable-line react-hooks/exhaustive-deps, react-hooks/set-state-in-effect

  const [preview, setPreview] = useState(false)
  const [diffCursor, setDiffCursor] = useState(0)

  const sortedUnmatched = useMemo(() => [...unmatchedPages].sort((a, b) => a.page_index - b.page_index), [unmatchedPages])
  const diffGroups = useMemo(() => {
    const grps: UnmatchedPage[][] = []
    for (const p of sortedUnmatched) {
      const last = grps[grps.length - 1]
      if (last && p.page_index === last[last.length - 1].page_index + 1) last.push(p); else grps.push([p])
    }
    return grps
  }, [sortedUnmatched])

  const placedCount = useMemo(() => (groups.target ?? []).filter(isSourceItem).length, [groups.target])

  function scrollToDiff(di: number) {
    const cl = Math.max(0, Math.min(diffGroups.length - 1, di)); setDiffCursor(cl)
    const pg = diffGroups[cl]?.[0]; if (!pg) return
    const sr = sourceRowRef.current; const tr = targetRowRef.current
    // 在源行或目标行找到差异点元素，计算其在行内的百分比位置
    let el = sr?.querySelector<HTMLElement>(`[data-source-page="${pg.page_index}"]`) ?? null
    let container: HTMLElement | null = null
    if (el) container = sr
    else if (tr) {
      const pid = pageId(pg.page_index); const arr = groups.target ?? []; const idx = arr.indexOf(pid)
      if (idx >= 0) for (let j = idx - 1; j >= 0; j--) if (arr[j].startsWith("target-")) { el = tr.querySelector<HTMLElement>(`[data-target-page="${arr[j].replace("target-", "")}"]`); container = tr; break }
    }
    if (!el || !container) return
    const pct = (el.offsetLeft + el.clientWidth / 2) / container.scrollWidth
    // 两行都滚到相同百分比
    const sl = sourceLenisRef.current?.lenis; const tl = targetLenisRef.current?.lenis
    if (sr) { const t = Math.max(0, pct * sr.scrollWidth - sr.clientWidth / 2); if (sl) sl.scrollTo(t); else sr.scrollTo({ left: t, behavior: "instant" as ScrollBehavior }) }
    if (tr) { const t = Math.max(0, pct * tr.scrollWidth - tr.clientWidth / 2); if (tl) tl.scrollTo(t); else tr.scrollTo({ left: t, behavior: "instant" as ScrollBehavior }) }
  }

  function buildInsertions(): ExportPatchInsertion[] {
    const result: ExportPatchInsertion[] = []
    const target = groups.target ?? []
    let currentAfter: null | number = null
    let pages: number[] = []
    for (const id of target) {
      if (isSourceItem(id)) {
        pages.push(Number(id.replace("source-", "")))
      } else {
        if (pages.length > 0) {
          result.push({ after_filename: currentAfter === null ? null : targetPages[currentAfter] ?? null, before_filename: null, page_indices: pages.sort((a, b) => a - b) })
          pages = []
        }
        currentAfter = Number(id.replace("target-", ""))
      }
    }
    if (pages.length > 0) result.push({ after_filename: currentAfter === null ? null : targetPages[currentAfter] ?? null, before_filename: null, page_indices: pages.sort((a, b) => a - b) })
    return result
  }

  return (
    <div style={{ position: "fixed", inset: 0, overflow: "hidden", boxSizing: "border-box", zIndex: 9600, background: "rgba(0,0,0,0.96)", display: "flex", flexDirection: "column", padding: 16, color: "#fff" }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <h3 style={{ margin: 0 }}>{t("Arrange Extra Pages")}</h3>
        <IconButton icon="fa fa-times" onClick={onCancel} size={32} className="modal-close-btn" style={{ borderRadius: "50%" }} />
      </div>
      <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, marginTop: 4 }}>
        {t("Drag the outlined pages onto the row below to place them; {{placed}} of {{total}} placed.", { placed: placedCount, total: unmatchedPages.length })}
      </div>

      {diffGroups.length > 0 && (
        <div style={{ display: "flex", alignItems: "center", justifyContent: "center", gap: 8, marginTop: 8 }}>
          <button type="button" className="stdbtn" onClick={() => scrollToDiff(diffCursor - 1)} disabled={diffCursor === 0}>
            <i className="fa fa-chevron-left" aria-hidden="true" style={{ marginRight: 4 }} />{t("Previous difference")}
          </button>
          <span style={{ fontSize: FONT_SIZE_SM, opacity: 0.8 }}>{t("{{current}} / {{total}}", { current: diffCursor + 1, total: diffGroups.length })}</span>
          <button type="button" className="stdbtn" onClick={() => scrollToDiff(diffCursor + 1)} disabled={diffCursor === diffGroups.length - 1}>
            {t("Next difference")}<i className="fa fa-chevron-right" aria-hidden="true" style={{ marginLeft: 4 }} />
          </button>
        </div>
      )}

      <DragDropProvider
        onDragStart={() => { sourceLenisRef.current?.lenis.stop(); targetLenisRef.current?.lenis.stop() }}
        onDragOver={(event) => { if (event.operation.source?.type !== "column") setGroups((g) => move(g, event)) }}
        onDragEnd={() => { sourceLenisRef.current?.lenis.start(); targetLenisRef.current?.lenis.start() }}
      >
        <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column", gap: 16, overflowY: "auto", marginTop: 16 }}>
          {/* 源行 */}
          <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
            <div style={{ height: 28, display: "flex", alignItems: "center", justifyContent: "space-between", marginBottom: 4 }}>
              <span style={{ fontSize: FONT_SIZE_SM, opacity: 0.7 }}>{t("Source pages (dashed = extra, drag onto the row below)")}</span>
              <button type="button" className="stdbtn" onClick={() => setGroups(buildDefaultGroups())} style={{ display: "flex", alignItems: "center", gap: 4 }}>
                <i className="fa fa-robot" aria-hidden="true" />{t("AI Suggested Positions")}
              </button>
            </div>
            <Slot id="source">
              <ScrollRow count={sourceTotalPages} rowRef={sourceRowRef} lenisApiRef={sourceLenisRef}
                renderPage={(index) => {
                  if (!unmatchedIndices.has(index)) return <SourcePlain key={index} index={index} queueItemId={queueItemId} side={sourceSide} />
                  const id = pageId(index)
                  const inSource = (groups.source ?? []).includes(id)
                  if (inSource) return <SourceDrag key={index} id={id} index={(groups.source ?? []).indexOf(id)} queueItemId={queueItemId} side={sourceSide} />
                  return <SourceGhost key={index} index={index} queueItemId={queueItemId} side={sourceSide} />
                }}
              />
            </Slot>
          </div>

          {/* 目标行 */}
          <div style={{ flex: 1, minHeight: 0, display: "flex", flexDirection: "column" }}>
            <div style={{ height: 28, display: "flex", alignItems: "center", fontSize: FONT_SIZE_SM, opacity: 0.7, marginBottom: 4 }}>
              {t("Target pages (drop zone between each page)")}
            </div>
            <Slot id="target">
            <ScrollRow count={1} rowRef={targetRowRef} lenisApiRef={targetLenisRef}
              renderPage={() => (
                <div key="target-row" style={{ display: "flex", gap: 6, height: "100%" }}>
                  {(groups.target ?? []).map((id, i) => isSourceItem(id)
                    ? <TargetDrag key={id} id={id} index={i} queueItemId={queueItemId} side={sourceSide} />
                    : <TargetThumbSortable key={id} id={id} index={i} queueItemId={queueItemId} side={targetSide} />
                  )}
                </div>
              )}
            />
            </Slot>
          </div>
        </div>

        <DragOverlay>
          {(source) => {
            const idx = Number(String(source.id).replace("source-", ""))
            return <div style={{ width: 150, aspectRatio: THUMB_ASPECT_RATIO, borderRadius: 4, overflow: "hidden", boxShadow: "0 4px 12px rgba(0,0,0,0.5)" }}>
              <img src={`/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${sourceSide}&index=${idx}`} alt=""
                style={{ width: "100%", height: "100%", objectFit: "cover", display: "block" }} />
            </div>
          }}
        </DragOverlay>
      </DragDropProvider>

      <div style={{ display: "flex", justifyContent: "flex-end", gap: 8, marginTop: 16 }}>
        {!preview ? (<>
          <button type="button" className="stdbtn" onClick={onCancel}>{t("Cancel")}</button>
          <button type="button" className="stdbtn" onClick={() => setPreview(true)}>{t("Preview")}</button>
        </>) : (<>
          <button type="button" className="stdbtn" onClick={() => setPreview(false)}>{t("Back")}</button>
          <button type="button" className="stdbtn stdbtn-danger" onClick={() => onConfirm(buildInsertions())}>{t("Done")}</button>
        </>)}
      </div>

      {preview && (
        <div style={{ marginTop: 12, padding: 12, background: "rgba(255,255,255,0.06)", borderRadius: 6, maxHeight: "30vh", overflowY: "auto" }}>
          <div style={{ fontSize: FONT_SIZE_SM, fontWeight: 700, marginBottom: 6 }}>{t("Preview")}</div>
          {placedCount === 0 ? <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.7 }}>{t("No extra pages placed — nothing will be added.")}</div>
            : [...unmatchedByIndex.keys()].filter((i) => !(groups.source ?? []).includes(pageId(i))).map((i) => {
              const pl = (groups.target ?? []).indexOf(targetId(i))
              return <div key={i} style={{ fontSize: FONT_SIZE_SM, opacity: 0.85 }}>{t("Source page {{n}} → {{where}}", { n: i + 1, where: pl >= 0 ? t("after page {{n}}", { n: pl + 1 }) : t("start") })}</div>
            })}
        </div>
      )}
    </div>
  )
}
