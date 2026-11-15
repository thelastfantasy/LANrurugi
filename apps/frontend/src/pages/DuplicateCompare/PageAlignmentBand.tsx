import { useRef } from "react"
import { useTranslation } from "react-i18next"

import type { ComparisonSide, DuplicateCompareSample } from "@/api/types"
import { FONT_SIZE_XS } from "@/theme"

import { SideBadge } from "./SideBadge"
import { SIDE_COLORS } from "./sideColors"

const MATCHED = "rgba(255,255,255,0.22)"
const ONLY_HERE = "#e67e22"
const SAMPLE = "rgba(255,255,255,0.85)"
const PATCHED = "#9b7be0"

/** "3, 7–9, 12" from 0-based page indexes, shown 1-based. */
export function pageRanges(indexes: number[]): string {
  const sorted = [...indexes].sort((x, y) => x - y)
  const parts: string[] = []
  let start = -1
  let prev = -2
  for (const i of [...sorted, Number.NaN]) {
    if (i === prev + 1) {
      prev = i
      continue
    }
    if (start >= 0) parts.push(start === prev ? `${start + 1}` : `${start + 1}–${prev + 1}`)
    start = i
    prev = i
  }
  return parts.join(", ")
}

/** Where a band pick lands: a sample, or (for a page only one side has) that page's tile. */
export type JumpTarget = { kind: "sample"; index: number } | { kind: "only"; side: ComparisonSide; page: number }

/** Index of the sample whose page on `side` is nearest `page`. */
function nearestSample(samples: DuplicateCompareSample[], side: ComparisonSide, page: number): number {
  let best = 0
  let bestDistance = Number.POSITIVE_INFINITY
  samples.forEach((sample, i) => {
    const distance = Math.abs((side === "a" ? sample.a_page_index : sample.b_page_index) - page)
    if (distance < bestDistance) {
      best = i
      bestDistance = distance
    }
  })
  return best
}

/** Whole-book overview, one cell per page per side. The row is one pointer target, not a button
 *  per page: a 200-page book leaves each cell ~1px wide on a phone, so the page comes from the
 *  pointer's position, and dragging along the row scrubs through the samples. A page only this
 *  side has jumps to its own tile instead of the nearest sample. */
export function PageAlignmentBand({
  aPages,
  bPages,
  aUnmatched,
  bUnmatched,
  aPatch,
  bPatch,
  samples,
  current,
  compact,
  onJump,
}: {
  aPages: number
  bPages: number
  aUnmatched: number[]
  bUnmatched: number[]
  /** Pages that come from each side's `.patch.zip`. */
  aPatch: number[]
  bPatch: number[]
  samples: DuplicateCompareSample[]
  current: number | null
  compact: boolean
  onJump: (target: JumpTarget) => void
}) {
  const { t } = useTranslation()
  const rows: { side: ComparisonSide; pages: number; unmatched: Set<number>; patched: Set<number>; sampled: Map<number, number> }[] = [
    { side: "a", pages: aPages, unmatched: new Set(aUnmatched), patched: new Set(aPatch), sampled: new Map(samples.map((s, i) => [s.a_page_index, i])) },
    { side: "b", pages: bPages, unmatched: new Set(bUnmatched), patched: new Set(bPatch), sampled: new Map(samples.map((s, i) => [s.b_page_index, i])) },
  ]

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: compact ? 6 : 4 }}>
      {rows.map((row) => (
        <BandRow key={row.side} {...row} samples={samples} current={current} compact={compact} onJump={onJump} />
      ))}
      <div style={{ display: "flex", flexWrap: "wrap", columnGap: 12, rowGap: 2, fontSize: FONT_SIZE_XS, opacity: 0.8, paddingLeft: 26 }}>
        <Legend color={MATCHED} label={t("duplicates.legendMatched")} />
        <Legend color={ONLY_HERE} label={t("duplicates.legendOnlyHere")} />
        <Legend color={SAMPLE} label={t("duplicates.legendSample")} />
        {aPatch.length + bPatch.length > 0 && <Legend color={PATCHED} label={t("duplicates.legendPatch")} />}
        {aUnmatched.length > 0 && <span>{t("duplicates.onlyInSide", { side: "A", pages: pageRanges(aUnmatched) })}</span>}
        {bUnmatched.length > 0 && <span>{t("duplicates.onlyInSide", { side: "B", pages: pageRanges(bUnmatched) })}</span>}
      </div>
    </div>
  )
}

function BandRow({
  side,
  pages,
  unmatched,
  patched,
  sampled,
  samples,
  current,
  compact,
  onJump,
}: {
  side: ComparisonSide
  pages: number
  unmatched: Set<number>
  patched: Set<number>
  sampled: Map<number, number>
  samples: DuplicateCompareSample[]
  current: number | null
  compact: boolean
  onJump: (target: JumpTarget) => void
}) {
  const { t } = useTranslation()
  const trackRef = useRef<HTMLDivElement | null>(null)
  const lastJump = useRef<string | null>(null)
  const enabled = pages > 0 && (samples.length > 0 || unmatched.size > 0)
  const currentSample = current === null ? undefined : samples[current]
  const currentPage = side === "a" ? currentSample?.a_page_index : currentSample?.b_page_index

  function jumpAt(clientX: number) {
    const track = trackRef.current
    if (!track || !enabled) return
    const rect = track.getBoundingClientRect()
    const fraction = Math.min(0.999, Math.max(0, (clientX - rect.left) / rect.width))
    const page = Math.floor(fraction * pages)
    const sample = sampled.get(page)
    let target: JumpTarget
    if (sample !== undefined) target = { kind: "sample", index: sample }
    else if (unmatched.has(page)) target = { kind: "only", side, page }
    else if (samples.length > 0) target = { kind: "sample", index: nearestSample(samples, side, page) }
    else return
    const key = target.kind === "sample" ? `s${target.index}` : `o${target.page}`
    if (key === lastJump.current) return
    lastJump.current = key
    onJump(target)
  }

  return (
    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
      <SideBadge side={side} size={18} />
      <div
        ref={trackRef}
        role="slider"
        aria-label={t("duplicates.alignmentBand")}
        aria-valuemin={1}
        aria-valuemax={pages}
        aria-valuenow={currentPage === undefined ? undefined : currentPage + 1}
        onPointerDown={(e) => {
          if (!enabled) return
          e.currentTarget.setPointerCapture(e.pointerId)
          lastJump.current = null
          jumpAt(e.clientX)
        }}
        onPointerMove={(e) => {
          if (e.currentTarget.hasPointerCapture(e.pointerId)) jumpAt(e.clientX)
        }}
        style={{
          display: "flex",
          flex: 1,
          minWidth: 0,
          // Taller on touch screens: the whole row is the finger target.
          height: compact ? 24 : 14,
          gap: pages > 120 ? 0 : 1,
          alignItems: "center",
          cursor: enabled ? "pointer" : "default",
          touchAction: "none",
        }}
      >
        {Array.from({ length: pages }, (_, page) => {
          const sampleIndex = sampled.get(page)
          const isCurrent = sampleIndex !== undefined && sampleIndex === current
          return (
            <span
              key={page}
              style={{
                flex: 1,
                minWidth: 0,
                borderRadius: 1,
                background: isCurrent
                  ? SIDE_COLORS[side]
                  : sampleIndex !== undefined
                    ? SAMPLE
                    : unmatched.has(page)
                      ? ONLY_HERE
                      : patched.has(page)
                        ? PATCHED
                        : MATCHED,
                height: sampleIndex !== undefined ? "100%" : "60%",
                // A sampled page stays visible even when the row has more pages than pixels.
                minInlineSize: sampleIndex !== undefined ? 2 : 0,
              }}
            />
          )
        })}
      </div>
    </div>
  )
}

function Legend({ color, label }: { color: string; label: string }) {
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 4 }}>
      <span style={{ width: 10, height: 10, borderRadius: 2, background: color, display: "inline-block" }} />
      {label}
    </span>
  )
}
