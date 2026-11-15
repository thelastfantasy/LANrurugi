import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { duplicateCompareQuery, useArchivePages } from "@/api/hooks"
import type { DuplicateCompare, DuplicateGroupView } from "@/api/types"
import { formatBytes } from "@/components/Display"
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"

import { LetterBadge } from "./SideBadge"
import { memberColor, memberLetter } from "./sideColors"

const SAMPLE_ROWS = 6
const LENS_ZOOM = 2.5
const TOUCH_HOLD_MS = 150
const SCROLL_INTENT_PX = 10

type Result = { data?: DuplicateCompare; error?: string }

/** Runs anchor-vs-member comparisons one at a time (each reads two whole archives), sharing the
 *  pair view's query cache so neither repeats the other's work. */
function useAnchorComparisons(anchor: string, others: string[], refreshKey: number): Record<string, Result> {
  const queryClient = useQueryClient()
  const [results, setResults] = useState<Record<string, Result>>({})
  const key = `${anchor}|${others.join(",")}|${refreshKey}`

  useEffect(() => {
    let cancelled = false
    setResults({})
    void (async () => {
      for (const other of others) {
        if (cancelled) return
        try {
          const data = await queryClient.fetchQuery(duplicateCompareQuery(anchor, other))
          if (!cancelled) setResults((r) => ({ ...r, [other]: { data } }))
        } catch (error) {
          if (!cancelled) setResults((r) => ({ ...r, [other]: { error: String(error) } }))
        }
      }
    })()
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, queryClient])
  return results
}

/** Average of one side's sampled sharpness. */
function meanSharpness(data: DuplicateCompare, side: "a" | "b"): number | null {
  if (data.samples.length === 0) return null
  const sum = data.samples.reduce((s, x) => s + (side === "a" ? x.a_sharpness : x.b_sharpness), 0)
  return sum / data.samples.length
}

/** Anchor pages to show as rows: aligned in as many members as possible, spread over the book. */
function pickRows(anchorPages: number, maps: Map<number, number>[]): number[] {
  if (anchorPages === 0 || maps.length === 0) return []
  const coverage = Array.from({ length: anchorPages }, (_, page) => maps.filter((m) => m.has(page)).length)
  const best = Math.max(...coverage)
  const candidates = coverage.flatMap((c, page) => (c === best && c > 0 ? [page] : []))
  if (candidates.length <= SAMPLE_ROWS) return candidates
  return Array.from({ length: SAMPLE_ROWS }, (_, i) =>
    candidates[Math.round((i * (candidates.length - 1)) / (SAMPLE_ROWS - 1))],
  )
}

interface Column {
  id: string
  index: number
  title: string
}

/**
 * Every member against one anchor (A): a table of what each comparison found, with a keep box per
 * member, then sample pages with every member's matching page stacked in one place. Holding
 * member N's button or digit key shows that copy, releasing returns to A, like Shift in the pair
 * view. Executing keeps the ticked members and recycles the rest.
 */
export function MultiCompare({
  group,
  anchor,
  narrow,
  held,
  onHeld,
  refreshKey,
  busy,
  onAnchor,
  onPatch,
  onExecute,
}: {
  group: DuplicateGroupView
  anchor: string
  narrow: boolean
  /** Column index being shown in every row instead of A, while its button or digit key is held. */
  held: number | null
  onHeld: (column: number | null) => void
  refreshKey: number
  busy: boolean
  onAnchor: (member: string) => void
  /** Patch `from`'s pages that `into` lacks into `into`. */
  onPatch: (from: string, into: string) => void
  onExecute: (decisions: Record<string, "keep" | "discard">, notDuplicate: boolean) => void
}) {
  const { t } = useTranslation()
  const others = useMemo(() => group.members.map((m) => m.id).filter((id) => id !== anchor), [group.members, anchor])
  const results = useAnchorComparisons(anchor, others, refreshKey)
  const [keep, setKeep] = useState(() => initialKeep(group))
  const [confirming, setConfirming] = useState(false)
  const columns: Column[] = [anchor, ...others].map((id, index) => ({
    id,
    index,
    title: group.members.find((m) => m.id === id)?.title ?? id,
  }))
  const done = others.filter((id) => results[id]).length
  const anyData = others.map((id) => results[id]?.data).find(Boolean)
  const anchorPages = anyData?.a.pages ?? group.members.find((m) => m.id === anchor)?.pages ?? 0

  // a-page -> b-page per loaded member.
  const maps: Record<string, Map<number, number>> = {}
  for (const id of others) {
    const aligned = results[id]?.data?.aligned
    if (aligned) maps[id] = new Map(aligned)
  }
  const rows = pickRows(anchorPages, Object.values(maps))
  const anchorValues = others.flatMap((id) => {
    const data = results[id]?.data
    const v = data ? meanSharpness(data, "a") : null
    return v === null ? [] : [v]
  })
  const anchorSharpness = anchorValues.length ? anchorValues.reduce((a, b) => a + b, 0) / anchorValues.length : null

  const [lens, setLens] = useState<{ row: number; fx: number; fy: number } | null>(null)
  const columnCount = columns.length
  const heldRef = useRef(onHeld)
  heldRef.current = onHeld
  useEffect(() => {
    function digit(e: KeyboardEvent): number | null {
      if (e.ctrlKey || e.metaKey || e.altKey) return null
      const target = e.target as HTMLElement | null
      if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)) return null
      const d = /^[1-9]$/.test(e.key) ? Number(e.key) - 1 : null
      return d !== null && d < columnCount ? d : null
    }
    let down: number | null = null
    function onDown(e: KeyboardEvent) {
      const d = digit(e)
      if (d === null) return
      down = d
      heldRef.current(d)
    }
    function onUp(e: KeyboardEvent) {
      const d = digit(e)
      if (d !== null && d === down) {
        down = null
        heldRef.current(null)
      }
    }
    const onBlur = () => heldRef.current(null)
    window.addEventListener("keydown", onDown)
    window.addEventListener("keyup", onUp)
    window.addEventListener("blur", onBlur)
    return () => {
      window.removeEventListener("keydown", onDown)
      window.removeEventListener("keyup", onUp)
      window.removeEventListener("blur", onBlur)
    }
  }, [columnCount])

  const decisions: Record<string, "keep" | "discard"> = {}
  for (const column of columns) decisions[column.id] = keep.has(column.id) ? "keep" : "discard"
  const keepCount = columns.filter((c) => keep.has(c.id)).length
  const discardCount = columns.length - keepCount
  const suggested = suggestedKeep(group)
  const followsSuggestion = suggested !== null && columns.every((c) => suggested.has(c.id) === keep.has(c.id))

  // Pages a recycled copy has that a kept one lacks. Known only against the anchor: A's pages a
  // kept member misses when A goes, or a recycled member's pages A misses when A stays.
  const losses: { from: string; into: string; count: number }[] = []
  if (!keep.has(anchor)) {
    for (const id of others) {
      const count = results[id]?.data?.a_unmatched_pages.length ?? 0
      if (keep.has(id) && count > 0) losses.push({ from: anchor, into: id, count })
    }
  } else {
    for (const id of others) {
      const count = results[id]?.data?.b_unmatched_pages.length ?? 0
      if (!keep.has(id) && count > 0) losses.push({ from: id, into: anchor, count })
    }
  }
  const letterOf = (id: string) => memberLetter(columns.find((c) => c.id === id)?.index ?? 0)

  const cell = { padding: "4px 6px", whiteSpace: "nowrap" as const }
  return (
    <>
      <div style={{ flex: 1, minHeight: 0, overflowY: "auto", overscrollBehavior: "contain", paddingBottom: 12 }}>
        <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
          {done < others.length && (
            <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8 }}>
              <i className="fa fa-spinner fa-spin" aria-hidden="true" style={{ marginRight: 6 }} />
              {t("duplicates.multiProgress", { done, total: others.length })}
            </div>
          )}

          <div style={{ overflowX: "auto", fontSize: FONT_SIZE_SM }}>
            <table style={{ width: "100%", borderCollapse: "collapse", minWidth: 620 }}>
              <thead>
                <tr style={{ opacity: 0.7, fontSize: FONT_SIZE_XS, textAlign: "left" }}>
                  <th style={cell}>{t("duplicates.multiKeep")}</th>
                  <th style={cell} />
                  <th style={cell}>{t("duplicates.title")}</th>
                  <th style={cell}>{t("duplicates.pages")}</th>
                  <th style={cell}>{t("duplicates.filesize")}</th>
                  <th style={cell}>{t("duplicates.multiAligned")}</th>
                  <th style={cell}>{t("duplicates.multiOnlyHere")}</th>
                  <th style={cell}>{t("duplicates.multiMissing")}</th>
                  <th style={cell}>{t("duplicates.multiSharpness")}</th>
                  <th style={cell} />
                </tr>
              </thead>
              <tbody>
                {columns.map((column) => {
                  const member = group.members.find((m) => m.id === column.id)
                  const result = column.index === 0 ? undefined : results[column.id]
                  const data = result?.data
                  const pending = column.index > 0 && !result
                  const dash = pending ? <i className="fa fa-spinner fa-spin" aria-hidden="true" /> : "—"
                  const sharpness = column.index === 0 ? anchorSharpness : data ? meanSharpness(data, "b") : null
                  const better = data?.recommendation === "b"
                  return (
                    <tr
                      key={column.id}
                      style={{ borderTop: "1px solid rgba(255,255,255,0.1)", opacity: keep.has(column.id) ? 1 : 0.55 }}
                    >
                      <td style={{ ...cell, width: 32, textAlign: "center" }}>
                        <input
                          type="checkbox"
                          checked={keep.has(column.id)}
                          aria-label={t("duplicates.multiKeep")}
                          onChange={() =>
                            setKeep((current) => {
                              const next = new Set(current)
                              if (next.has(column.id)) next.delete(column.id)
                              else next.add(column.id)
                              return next
                            })
                          }
                          style={{ width: 20, height: 20, margin: 0 }}
                        />
                      </td>
                      <td style={{ ...cell, width: 24 }}>
                        <LetterBadge letter={memberLetter(column.index)} color={memberColor(column.index)} />
                      </td>
                      <td style={{ padding: "4px 6px", maxWidth: 320 }}>
                        <div style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={column.title}>
                          {member?.recommended && <i className="fa fa-star" aria-hidden="true" style={{ marginRight: 4 }} />}
                          {column.title}
                        </div>
                        {group.review?.notes?.[column.id] && !group.review.fallback && (
                          <div
                            style={{ fontSize: FONT_SIZE_XS, opacity: 0.7, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}
                            title={group.review.notes[column.id]}
                          >
                            <i className="fa fa-robot" aria-hidden="true" style={{ marginRight: 4 }} />
                            {group.review.notes[column.id]}
                          </div>
                        )}
                        {result?.error && <div style={{ fontSize: FONT_SIZE_XS, color: "#ff8a80" }}>{result.error}</div>}
                      </td>
                      <td style={cell}>{data?.b.pages ?? member?.pages ?? "—"}</td>
                      <td style={cell}>{member ? formatBytes(member.size) : "—"}</td>
                      <td style={cell}>{column.index === 0 ? "—" : data ? `${data.aligned_pairs}` : dash}</td>
                      <td style={{ ...cell, color: data && data.b_unmatched_pages.length > 0 ? "#e67e22" : undefined }}>
                        {column.index === 0 ? "—" : data ? data.b_unmatched_pages.length : dash}
                      </td>
                      <td style={{ ...cell, color: data && data.a_unmatched_pages.length > 0 ? "#e67e22" : undefined }}>
                        {column.index === 0 ? "—" : data ? data.a_unmatched_pages.length : dash}
                      </td>
                      <td style={cell}>
                        {sharpness === null ? (column.index === 0 && pending ? dash : "—") : sharpness.toFixed(0)}
                        {better && <i className="fa fa-arrow-up" aria-hidden="true" title={t("duplicates.multiSharperThanA")} style={{ marginLeft: 4, color: "#7bd88f" }} />}
                      </td>
                      <td style={{ ...cell, textAlign: "right" }}>
                        {column.index > 0 && (
                          <button type="button" className="stdbtn" style={{ margin: 0, minHeight: 32 }} onClick={() => onAnchor(column.id)}>
                            {t("duplicates.setAsA")}
                          </button>
                        )}
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
            <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.6, marginTop: 4 }}>{t("duplicates.multiTableHint")}</div>
          </div>

          {rows.length > 0 && (
            <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.7, textAlign: "center" }}>
              <i className="fa fa-info-circle" aria-hidden="true" style={{ marginRight: 4 }} />
              {t(narrow ? "duplicates.multiHintTouch" : "duplicates.multiHint")}
            </div>
          )}

          {rows.map((anchorPage, row) => (
            <StackedCell
              key={anchorPage}
              layers={columns.map((column) => {
                const page = column.index === 0 ? anchorPage : maps[column.id]?.get(anchorPage)
                const patchPages = column.index === 0 ? anyData?.a_patch_pages : results[column.id]?.data?.b_patch_pages
                return {
                  archiveId: column.id,
                  letter: memberLetter(column.index),
                  color: memberColor(column.index),
                  page,
                  pending: column.index > 0 && !results[column.id],
                  fromPatch: page !== undefined && (patchPages ?? []).includes(page),
                }
              })}
              shown={held ?? 0}
              lens={lens?.row === row ? lens : null}
              narrow={narrow}
              onLens={(fx, fy) => setLens(fx === null || fy === null ? null : { row, fx, fy })}
            />
          ))}
        </div>
      </div>

      <div
        style={{
          display: "grid",
          gridTemplateColumns: narrow ? "1fr 1fr" : "auto auto auto",
          justifyContent: narrow ? undefined : "end",
          gap: 8,
          borderTop: "1px solid rgba(255,255,255,0.15)",
          padding: narrow ? "8px 0 max(8px, env(safe-area-inset-bottom))" : "8px 0 0",
        }}
      >
        <button
          type="button"
          className="stdbtn"
          disabled={busy || keepCount === 0 || discardCount === 0}
          onClick={() => setConfirming(true)}
          title={followsSuggestion ? t("duplicates.recommended") : undefined}
          style={{
            ...footerButton(narrow),
            gridColumn: narrow ? "1 / -1" : undefined,
            fontWeight: 700,
            boxShadow: followsSuggestion ? "0 0 0 2px rgba(255,255,255,0.85)" : undefined,
          }}
        >
          {followsSuggestion && <i className="fa fa-star" aria-hidden="true" />}
          {t("duplicates.multiExecute", { keep: keepCount, discard: discardCount })}
        </button>
        <button
          type="button"
          className="stdbtn"
          disabled={busy}
          onClick={() => onExecute(Object.fromEntries(columns.map((c) => [c.id, "keep"])), false)}
          style={footerButton(narrow)}
        >
          {t("duplicates.multiKeepAll")}
        </button>
        <button type="button" className="stdbtn" disabled={busy} onClick={() => onExecute({}, true)} style={footerButton(narrow)}>
          {t("duplicates.notDuplicate")}
        </button>
      </div>

      {confirming && (
        <div
          role="dialog"
          aria-modal="true"
          onClick={() => setConfirming(false)}
          style={{ position: "fixed", inset: 0, zIndex: 9600, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}
        >
          <div
            onClick={(e) => e.stopPropagation()}
            style={{ background: "#1d1d21", color: "#f2f2f2", borderRadius: 8, padding: 16, maxWidth: 480, width: "100%", maxHeight: "90dvh", overflowY: "auto", boxShadow: "0 8px 24px rgba(0,0,0,0.5)" }}
          >
            <div style={{ fontWeight: 700, marginBottom: 8 }}>
              {t("duplicates.multiConfirmTitle", { keep: keepCount, discard: discardCount })}
            </div>
            <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.85, marginBottom: 6 }}>{t("duplicates.multiConfirmBody")}</div>
            <div style={{ display: "flex", flexDirection: "column", gap: 4, marginBottom: 8 }}>
              {columns
                .filter((c) => !keep.has(c.id))
                .map((c) => (
                  <div key={c.id} style={{ display: "flex", alignItems: "center", gap: 6, fontSize: FONT_SIZE_SM, minWidth: 0 }}>
                    <LetterBadge letter={memberLetter(c.index)} color={memberColor(c.index)} size={18} />
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }} title={c.title}>
                      {c.title}
                    </span>
                  </div>
                ))}
            </div>
            {losses.map((loss) => (
              <div
                key={`${loss.from}>${loss.into}`}
                style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 8, fontSize: FONT_SIZE_SM, padding: "8px 10px", borderRadius: 6, background: "rgba(230,126,34,0.18)", marginBottom: 6 }}
              >
                <i className="fa fa-puzzle-piece" aria-hidden="true" />
                <span style={{ flex: 1, minWidth: 160 }}>
                  {t("duplicates.multiLossWarn", { from: letterOf(loss.from), into: letterOf(loss.into), count: loss.count })}
                </span>
                <button
                  type="button"
                  className="stdbtn"
                  disabled={busy}
                  style={{ margin: 0, minHeight: 36 }}
                  onClick={() => {
                    setConfirming(false)
                    onPatch(loss.from, loss.into)
                  }}
                >
                  {t("duplicates.multiPatchInto", { into: letterOf(loss.into) })}
                </button>
              </div>
            ))}
            <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
              <button
                type="button"
                className="stdbtn"
                disabled={busy}
                style={{ margin: 0, minHeight: 40, fontWeight: 700 }}
                onClick={() => {
                  setConfirming(false)
                  onExecute(decisions, false)
                }}
              >
                {t("duplicates.multiConfirmYes", { discard: discardCount })}
              </button>
              <button type="button" className="stdbtn" style={{ margin: 0, minHeight: 40 }} onClick={() => setConfirming(false)}>
                {t("common.cancel")}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  )
}

function footerButton(narrow: boolean) {
  return {
    margin: 0,
    minHeight: narrow ? 44 : 32,
    padding: "0 14px",
    display: "inline-flex",
    alignItems: "center",
    justifyContent: "center",
    gap: 6,
    fontSize: FONT_SIZE_SM,
    touchAction: "manipulation" as const,
  }
}

/** The model's keep set when it ruled on every member, else null. */
function suggestedKeep(group: DuplicateGroupView): Set<string> | null {
  const review = group.review
  if (!review || review.fallback) return null
  const judged = new Set([...review.keep, ...review.discard])
  if (!group.members.every((m) => judged.has(m.id))) return null
  return new Set(review.keep)
}

/** Ticks start from the saved decisions, else the model's suggestion, else everything kept. */
function initialKeep(group: DuplicateGroupView): Set<string> {
  const suggested = suggestedKeep(group)
  return new Set(
    group.members
      .filter((m) =>
        m.decision === "keep" ? true : m.decision === "discard" ? false : suggested ? suggested.has(m.id) : true,
      )
      .map((m) => m.id),
  )
}

interface Layer {
  archiveId: string
  letter: string
  color: string
  page: number | undefined
  pending: boolean
  fromPatch: boolean
}

/** One sample page with every member's matching page stacked on A's: all are loaded up front and
 *  only the shown one is visible, so switching is instant and differences read as flicker. */
function StackedCell({
  layers,
  shown,
  lens,
  narrow,
  onLens,
}: {
  layers: Layer[]
  shown: number
  lens: { fx: number; fy: number } | null
  narrow: boolean
  onLens: (fx: number | null, fy: number | null) => void
}) {
  const { t } = useTranslation()
  const boxRef = useRef<HTMLDivElement | null>(null)
  const [size, setSize] = useState<{ w: number; h: number } | null>(null)
  const layer = layers[shown] ?? layers[0]

  const latest = useRef(onLens)
  latest.current = onLens

  function fraction(clientX: number, clientY: number): [number, number] | null {
    const box = boxRef.current?.getBoundingClientRect()
    if (!box || box.width === 0 || box.height === 0) return null
    return [
      Math.min(1, Math.max(0, (clientX - box.left) / box.width)),
      Math.min(1, Math.max(0, (clientY - box.top) / box.height)),
    ]
  }

  // Touch: press and hold to show the lens, then drag it; a quick swipe still scrolls.
  useEffect(() => {
    const el = boxRef.current
    if (!el) return
    let timer: ReturnType<typeof setTimeout> | null = null
    let active = false
    let start: { x: number; y: number } | null = null
    const clear = () => {
      if (timer) clearTimeout(timer)
      timer = null
    }
    const onStart = (e: TouchEvent) => {
      const touch = e.touches[0]
      start = { x: touch.clientX, y: touch.clientY }
      clear()
      timer = setTimeout(() => {
        active = true
        const f = fraction(touch.clientX, touch.clientY)
        if (f) latest.current(f[0], f[1])
      }, TOUCH_HOLD_MS)
    }
    const onMove = (e: TouchEvent) => {
      const touch = e.touches[0]
      if (!active) {
        if (start && Math.hypot(touch.clientX - start.x, touch.clientY - start.y) > SCROLL_INTENT_PX) clear()
        return
      }
      e.preventDefault()
      const f = fraction(touch.clientX, touch.clientY)
      if (f) latest.current(f[0], f[1])
    }
    const onEnd = () => {
      clear()
      if (active) latest.current(null, null)
      active = false
    }
    el.addEventListener("touchstart", onStart, { passive: true })
    el.addEventListener("touchmove", onMove, { passive: false })
    el.addEventListener("touchend", onEnd, { passive: true })
    el.addEventListener("touchcancel", onEnd, { passive: true })
    return () => {
      clear()
      el.removeEventListener("touchstart", onStart)
      el.removeEventListener("touchmove", onMove)
      el.removeEventListener("touchend", onEnd)
      el.removeEventListener("touchcancel", onEnd)
    }
  }, [])

  const lensPx = narrow ? 120 : 170
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 4 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 6, fontSize: FONT_SIZE_XS, alignSelf: "stretch" }}>
        <LetterBadge letter={layer.letter} color={layer.color} size={18} active={shown > 0} />
        {layer.page !== undefined
          ? t("duplicates.pageN", { page: layer.page + 1 })
          : layer.pending
            ? "…"
            : t("duplicates.multiNoMatch")}
        {layer.fromPatch && <i className="fa fa-puzzle-piece" aria-hidden="true" title={t("duplicates.fromPatch")} />}
      </div>
      <div
        ref={boxRef}
        onMouseMove={(e) => {
          const f = fraction(e.clientX, e.clientY)
          if (f) onLens(f[0], f[1])
        }}
        onMouseLeave={() => onLens(null, null)}
        onContextMenu={(e) => e.preventDefault()}
        style={{
          position: "relative",
          maxWidth: "100%",
          cursor: "zoom-in",
          WebkitTouchCallout: "none",
          userSelect: "none",
          outline: shown > 0 ? `2px solid ${layer.color}` : undefined,
          outlineOffset: 2,
          borderRadius: 4,
        }}
      >
        {layers.map((l, index) => (
          <LayerImage
            key={l.archiveId}
            layer={l}
            base={index === 0}
            visible={index === shown}
            lens={index === shown && size ? lens : null}
            lensPx={lensPx}
            size={size}
            onSize={index === 0 ? setSize : undefined}
          />
        ))}
        {layer.page === undefined && (
          <div
            style={{
              position: "absolute",
              inset: 0,
              display: "flex",
              alignItems: "center",
              justifyContent: "center",
              textAlign: "center",
              padding: 12,
              background: "rgba(0,0,0,0.75)",
              fontSize: FONT_SIZE_SM,
              borderRadius: 4,
            }}
          >
            {layer.pending ? <i className="fa fa-spinner fa-spin" aria-hidden="true" /> : t("duplicates.multiNoMatchLong")}
          </div>
        )}
      </div>
    </div>
  )
}

/** One member's page in a stack. A's page (`base`) is in flow and sizes the box; the others are
 *  fitted inside it. Original bytes, not the reader's resized variant: this view judges quality. */
function LayerImage({
  layer,
  base,
  visible,
  lens,
  lensPx,
  size,
  onSize,
}: {
  layer: Layer
  base: boolean
  visible: boolean
  lens: { fx: number; fy: number } | null
  lensPx: number
  size: { w: number; h: number } | null
  onSize?: (size: { w: number; h: number }) => void
}) {
  const files = useArchivePages(layer.page === undefined ? null : layer.archiveId)
  if (layer.page === undefined) return null
  const src =
    files.data?.pages[layer.page]?.url?.replace("&optimize=1", "") ??
    `/api/archives/${encodeURIComponent(layer.archiveId)}/thumbnail?page=${layer.page + 1}`
  return (
    <>
      <img
        src={src}
        alt=""
        draggable={false}
        onLoad={onSize ? (e) => onSize({ w: e.currentTarget.clientWidth, h: e.currentTarget.clientHeight }) : undefined}
        style={
          base
            ? {
                display: "block",
                maxWidth: "100%",
                maxHeight: "min(62vh, 900px)",
                borderRadius: 4,
                background: "#000",
                visibility: visible ? "visible" : "hidden",
              }
            : {
                position: "absolute",
                inset: 0,
                width: "100%",
                height: "100%",
                objectFit: "contain",
                borderRadius: 4,
                background: "#000",
                visibility: visible ? "visible" : "hidden",
              }
        }
      />
      {lens && size && (
        <div
          style={{
            position: "absolute",
            left: lens.fx * size.w - lensPx / 2,
            top: lens.fy * size.h - lensPx / 2,
            width: lensPx,
            height: lensPx,
            borderRadius: "50%",
            border: "2px solid rgba(255,255,255,0.85)",
            boxShadow: "0 2px 10px rgba(0,0,0,0.5)",
            pointerEvents: "none",
            backgroundImage: `url("${src}")`,
            backgroundRepeat: "no-repeat",
            backgroundSize: `${size.w * LENS_ZOOM}px ${size.h * LENS_ZOOM}px`,
            backgroundPosition: `${lensPx / 2 - lens.fx * size.w * LENS_ZOOM}px ${lensPx / 2 - lens.fy * size.h * LENS_ZOOM}px`,
            zIndex: 1,
          }}
        />
      )}
    </>
  )
}

/** Header buttons, one per member: hold one to show that copy in every sample, release for A. */
export function HoldButtons({
  count,
  held,
  narrow,
  onHeld,
}: {
  count: number
  held: number | null
  narrow: boolean
  onHeld: (column: number | null) => void
}) {
  const { t } = useTranslation()
  const size = narrow ? 34 : 30
  return (
    <div style={{ display: "inline-flex", gap: 6, flexShrink: 0 }}>
      {Array.from({ length: count }, (_, index) => (
        <button
          key={index}
          type="button"
          aria-pressed={held === index}
          title={t("duplicates.holdToShow", { letter: memberLetter(index), key: index + 1 })}
          onPointerDown={(e) => {
            e.preventDefault()
            e.currentTarget.setPointerCapture(e.pointerId)
            onHeld(index)
          }}
          onPointerUp={() => onHeld(null)}
          onPointerCancel={() => onHeld(null)}
          onLostPointerCapture={() => onHeld(null)}
          onContextMenu={(e) => e.preventDefault()}
          style={{
            all: "unset",
            cursor: "pointer",
            touchAction: "none",
            userSelect: "none",
            WebkitTouchCallout: "none",
            borderRadius: "50%",
            transform: held === index ? "scale(1.12)" : undefined,
          }}
        >
          <LetterBadge letter={memberLetter(index)} color={memberColor(index)} size={size} active={held === index} />
        </button>
      ))}
    </div>
  )
}
