import { useLayoutEffect, useRef, useState } from "react"

import type { StatTag } from "../api/types"

/** Faithful port of jQCloud 2.0.3's real word-placement algorithm (`~/LANraragi/public/js/vendor/
 * jqcloud.min.js`, decompiled and verified line-by-line against the actual minified source — no
 * npm package ships jQCloud's source readably, and this app doesn't depend on jQuery at all, so a
 * from-scratch port is the only way to reproduce it rather than approximate it) — legacy's own
 * `/stats` page (`stats.js`) calls `$("#tagCloud").jQCloud(data, { autoResize: true })` with no
 * other options, so every default here (`steps: 10`, `shape: 'elliptic'`, `center: {x:.5,y:.5}`,
 * `classPattern: 'w{n}'`, `removeOverflowing: true`, no `colors`/`fontSize` override) is the real
 * behavior, not a guess.
 *
 * What jQCloud actually does, reproduced here:
 * - Each word's *level* (1-10) comes from where its `weight` falls between the whole set's
 *   min/max, linearly: `round((weight-min)*(steps-1)/(max-min))+1`, or the middle step (5) if
 *   every weight is equal (`max===min`, division would be undefined).
 * - Font *size* is 100%/150%/…/550% for level 1-10 (`jqcloud.min.css`'s own real `.jqcloud-word.w1`
 *   through `.w10` rules) — not vendored into this app (nothing here ever loaded jQCloud's own
 *   CSS), so hardcoded here to match exactly.
 * - *Color* is NOT set here at all — each word gets class `jqcloud-word w{level}`, and every real
 *   theme file already vendored into this app (`public/legacy/themes/*.css`) has its own
 *   `div.jqcloud span.w{n}` color rules (verified: they override color only, never font-size),
 *   so plain class application is enough to pick up the exact right theme-matched color for free,
 *   including switching themes.
 * - *Placement*: each word starts at the container's exact center and spirals outward in 2-radian
 *   angular steps (elliptic shape's real `step` constant) with radius growing by the same amount
 *   each step, checking for bounding-box overlap against every already-placed word, until it finds
 *   a free spot — same center-distance AABB overlap test jQCloud's own `overlapping()` uses. Spiral
 *   direction alternates by word index parity (`index%2===0 ? +1 : -1`), an actual (if slightly
 *   arbitrary-looking) quirk of the real algorithm, not simplified away here. A word whose final
 *   placement extends past the container's own bounds is dropped entirely (`removeOverflowing`)
 *   rather than clipped or clamped — matching real jQCloud, this is why a container that's too
 *   small (or too few real tags to fill it) can visibly leave gaps or lose some words, which is
 *   exactly what a live report against this app's own from-scratch (word-wrap-list, no real
 *   placement) implementation flagged as "散布方式也不对（疑似数据不足或实装问题）" — it was in fact
 *   an implementation gap, not bad data.
 *
 * Requires a two-pass render: words first mount invisibly (real DOM nodes so their true rendered
 * size — including the actual font, not an estimate — can be measured via `getBoundingClientRect`)
 * before the spiral algorithm can run at all, since it needs each word's real pixel box to test
 * overlap against. `useLayoutEffect` (not `useEffect`) runs this measure-then-place pass before the
 * browser paints, so there's no visible flash of the unpositioned first pass.
 */

const STEPS = 10
// jQCloud's own real per-level font-size percentages (`.jqcloud-word.w1`..`.w10` in
// `jqcloud.min.css`) — index 0 is level 1.
const FONT_SIZE_PERCENT = [100, 150, 200, 250, 300, 350, 400, 450, 500, 550]
const SPIRAL_STEP = 2
const CENTER = { x: 0.5, y: 0.5 }

interface PlacedWord {
  tag: StatTag
  level: number
  left: number
  top: number
  width: number
  height: number
}

function levelFor(weight: number, min: number, max: number): number {
  if (max === min) return Math.floor(STEPS / 2)
  return Math.round(((weight - min) * (STEPS - 1)) / (max - min)) + 1
}

function overlaps(a: { left: number; top: number; width: number; height: number }, b: typeof a): boolean {
  return (
    Math.abs(2 * a.left + a.width - 2 * b.left - b.width) < a.width + b.width &&
    Math.abs(2 * a.top + a.height - 2 * b.top - b.height) < a.height + b.height
  )
}

export function TagCloud({ tags }: { tags: StatTag[] }) {
  const containerRef = useRef<HTMLDivElement>(null)
  const measureRefs = useRef<Map<string, HTMLSpanElement>>(new Map())
  const [placed, setPlaced] = useState<PlacedWord[] | null>(null)

  const sortedDesc = [...tags].sort((a, b) => b.weight - a.weight)
  const weights = sortedDesc.map((t) => t.weight)
  const maxWeight = weights[0] ?? 0
  const minWeight = weights[weights.length - 1] ?? 0

  function layout() {
    const container = containerRef.current
    if (!container || sortedDesc.length === 0) {
      setPlaced([])
      return
    }
    const width = container.clientWidth
    const height = container.clientHeight
    const aspectRatio = width / height
    // A single shared starting angle for every word's own spiral (matches jQCloud's own
    // `this.data.angle`, set once per layout run, not per word) — real, not a simplification.
    const baseAngle = 2 * Math.PI * Math.random()

    const result: PlacedWord[] = []
    sortedDesc.forEach((tag, index) => {
      const key = `${tag.namespace ?? ""}:${tag.text}`
      const el = measureRefs.current.get(key)
      if (!el) return
      const boxWidth = el.offsetWidth
      const boxHeight = el.offsetHeight

      let radius = 0
      let angle = baseAngle
      let box = {
        left: CENTER.x * width - boxWidth / 2,
        top: CENTER.y * height - boxHeight / 2,
        width: boxWidth,
        height: boxHeight,
      }
      // Spirals outward until no overlap against any word already placed this layout pass —
      // exactly jQCloud's own `drawOneWord` collision loop for the (default) elliptic shape.
      while (result.some((p) => overlaps(box, p))) {
        radius += SPIRAL_STEP
        angle += (index % 2 === 0 ? 1 : -1) * SPIRAL_STEP
        box = {
          left: CENTER.x * width - boxWidth / 2 + radius * Math.cos(angle) * aspectRatio,
          top: CENTER.y * height + radius * Math.sin(angle) - boxHeight / 2,
          width: boxWidth,
          height: boxHeight,
        }
      }

      // `removeOverflowing` (jQCloud's own default: true) — a word whose final box runs past the
      // container's own bounds is dropped entirely rather than clipped/clamped.
      if (box.left < 0 || box.top < 0 || box.left + box.width > width || box.top + box.height > height) {
        return
      }

      result.push({ tag, level: levelFor(tag.weight, minWeight, maxWeight), ...box })
    })
    setPlaced(result)
  }

  useLayoutEffect(() => {
    layout()
    const container = containerRef.current
    if (!container) return
    // `autoResize: true` in legacy's own real call — re-runs the same placement pass whenever the
    // container's own size actually changes (window resize, sidebar toggle, etc.), not just once
    // on mount.
    const observer = new ResizeObserver(() => layout())
    observer.observe(container)
    return () => observer.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tags])

  return (
    <div
      ref={containerRef}
      className="jqcloud"
      style={{
        position: "relative",
        width: "100%",
        height: "100%",
        overflow: "hidden",
        // jqcloud.min.css's own real `.jqcloud { font: 10px Helvetica,Arial,sans-serif; line-
        // height: normal }` — every word's `100%`-`550%` font-size below is relative to *this*
        // base, not the page's own ambient font-size, so it has to be reproduced exactly (a
        // missing/different base would scale the entire cloud uniformly wrong).
        font: "10px Helvetica, Arial, sans-serif",
        lineHeight: "normal",
      }}
    >
      {/* Invisible measurement pass — real DOM nodes (so `offsetWidth`/`offsetHeight` reflect the
          actual font/size, not an estimate) rendered off-screen until `layout()` has measured and
          positioned them. Kept mounted (not removed) after that so a later `ResizeObserver`-driven
          re-layout can re-measure the same real elements rather than needing a fresh mount cycle. */}
      <div style={{ position: "absolute", visibility: "hidden", pointerEvents: "none" }}>
        {sortedDesc.map((tag) => {
          const key = `${tag.namespace ?? ""}:${tag.text}`
          const level = levelFor(tag.weight, minWeight, maxWeight)
          return (
            <span
              key={key}
              ref={(el) => {
                if (el) measureRefs.current.set(key, el)
                else measureRefs.current.delete(key)
              }}
              className={`jqcloud-word w${level}`}
              style={{ fontSize: `${FONT_SIZE_PERCENT[level - 1]}%`, whiteSpace: "nowrap", display: "inline-block" }}
            >
              {tag.text}
            </span>
          )
        })}
      </div>

      {placed?.map((word) => {
        const key = `${word.tag.namespace ?? ""}:${word.tag.text}`
        return (
          <span
            key={key}
            className={`jqcloud-word w${word.level}`}
            style={{
              position: "absolute",
              left: word.left,
              top: word.top,
              fontSize: `${FONT_SIZE_PERCENT[word.level - 1]}%`,
              whiteSpace: "nowrap",
            }}
          >
            {word.tag.text}
          </span>
        )
      })}
    </div>
  )
}
