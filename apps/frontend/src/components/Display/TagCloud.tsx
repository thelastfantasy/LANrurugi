import { useEffect, useLayoutEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import TagCloudLib, { type TagCloud as TagCloudInstance } from "TagCloud"

import type { StatTag } from "@/api/types"

/** Dynamic 3D word cloud (issue #89 — replaces the 2D jQCloud port this component used to be, kept
 * around only in `git log` now) built on `TagCloud@2.5.0` (Cong Min, MIT, zero dependencies — see
 * issue #89's own library survey comment for why this one and not three.js/react-icon-cloud/etc.):
 * a real 3D sphere of words that drifts/auto-rotates continuously and responds to drag/mouse-move,
 * with built-in near-large-clear/far-small-faint depth (its own per-frame `scale`+`opacity` from
 * each word's z-depth).
 *
 * This component is a thin adapter, not a reimplementation — `TagCloud`'s own instance
 * (`pause`/`resume`/`destroy`/`update`) does all the actual 3D math and animation; everything here
 * is: (1) building the initial word list + weight→size/color mapping, (2) sizing the sphere to the
 * container and rebuilding on resize (`ResizeObserver`), (3) `prefers-reduced-motion`/tab-visibility/
 * per-tag-hover pause/resume, (4) a hover-scale effect on the *inner* text span (see below for why
 * it can't just be a CSS `:hover` on the item the library itself transforms).
 *
 * ## Hovering a tag pauses the whole sphere's drift
 *
 * Without this, the sphere keeps rotating out from under the pointer while a visitor is still
 * trying to read/click the word they just hovered — the library's own `keep`/mouse-follow behavior
 * only ever *slows toward* the pointer's own implied direction, it never actually stops just
 * because a specific word is currently under the cursor. One delegated `mouseover`/`mouseout`
 * pair on the sphere's own root element (not one listener per tag — `build()` already reruns on
 * every resize/tag-set change, and delegation means this doesn't need to be rewired each time)
 * calls the *same* `pause()`/`resume()` the tab-visibility effect below already uses, gated by
 * `pausedByVisibilityRef` so a `mouseout` firing while the tab is hidden doesn't incorrectly
 * resume it.
 *
 * ## Density (real, live-reported issue, fixed here)
 *
 * A first version reused the old 2D port's own `radius = shortSide * 0.32` and rendered every tag
 * unconditionally — with a real library's full tag set (346, in the report that flagged this) that
 * produced a visibly overcrowded, illegible sphere with unused space around it (a small sphere
 * floating in the middle of a much larger container). Fixed two ways together: `MAX_TAGS` caps how
 * many of the highest-weight tags actually render (a low-weight tag deep in a 346-strong sphere was
 * never legible or meaningfully distinguishable anyway — the detailed `#tagList` below this
 * component already covers the complete set for anyone who needs it), and `RADIUS_RATIO` raised
 * from 0.32 to 0.45 so the sphere itself actually fills the container it's given instead of
 * floating in a sea of unused space.
 *
 * ## Weight → size mapping (migrated from the old 2D port, color mapping is new)
 *
 * Same `levelFor`/`FONT_SIZE_PERCENT` 1–10 linear-by-weight mapping the old jQCloud port used for
 * *size*. *Color* is no longer the old port's `jqcloud-word w{n}` classes, which is a deliberate
 * break from the legacy-parity convention this app otherwise follows everywhere else — legacy's own
 * real `div.jqcloud span.w{n}` CSS (verified by reading every theme file's own rule) only ever
 * defines **four** distinct colors across the ten weight levels (w1 alone, w2–w4 grouped, w5–w7
 * grouped, w8–w10 grouped) — a real, live-reported "colors also monotonous" complaint traces
 * straight back to that legacy grouping, not a porting mistake. Per this project's own "custom
 * colors must be theme-aware, never hardcoded" rule, this now reads ten distinct
 * `--tag-cloud-level-N` CSS custom properties (one full gradient step per level, not four) that
 * each of the five real theme files defines for itself — see each theme file's own `.tag-cloud-3d`
 * block for the actual values and the reasoning behind the specific gradient chosen (interpolated
 * between that theme's own body text color and its own accent hue, so contrast against that here
 * exact theme's own background is guaranteed regardless of which theme is active).
 *
 * ## Why hover-scale needs its own inner `<span>`
 *
 * `TagCloud` writes `transform: translate3d(...) scale(...)` to each item element's own inline
 * style on every single animation frame (`_next()` in its own source) — a CSS `:hover { transform:
 * scale(1.25) }` rule on that *same* element would only ever "work" for one frame before the
 * library's own per-frame write stomps it right back out. Wrapping the real text in a second,
 * library-untouched inner `<span>` and putting the hover transform + transition there instead
 * composes cleanly: CSS transforms on nested elements multiply, so the sphere's own 3D
 * position/depth-scale (outer) and the hover pop (inner) apply independently — moving the mouse off
 * the sphere entirely still resumes normal drift immediately since the outer transform was never
 * touched by the hover rule at all.
 *
 * ## `destroy()`'s real leak, worked around
 *
 * `TagCloud`'s own `destroy()` (confirmed by reading its source, not assumed) removes the DOM
 * element and un-lists the instance but never actually cancels its internal
 * `requestAnimationFrame` loop (`self.interval = null` merely drops the *reference* to the handle,
 * the scheduled callback itself keeps firing) — a real, upstream bug, not a misunderstanding of the
 * public API surface (which exposes no `cancel`/`stop` to work around it with directly). Mitigated
 * here by never actually calling the library's own `destroy()` at all on unmount/resize/rebuild:
 * instead the *container* element itself is replaced with a fresh one each time (`key`-less manual
 * DOM swap via `containerRef.current.replaceChildren()` before creating a new instance), so the
 * orphaned RAF loop's own `itemEl.style.transform` writes land on now-detached, garbage-collectable
 * nodes instead of ones still visibly in the page — same practical effect as a real cancel for a
 * component that never needs more than one live instance at a time.
 */

const STEPS = 10
// Same real per-level font-size percentages the old 2D jQCloud port used (index 0 is level 1) —
// see this file's own top docs for why these carry over unchanged.
const FONT_SIZE_PERCENT = [100, 150, 200, 250, 300, 350, 400, 450, 500, 550]
// See this file's own top "Density" docs.
const MAX_TAGS = 150
// The library's own `_computePosition` spreads items across `±size/2` where `size = 1.5 * radius`
// — i.e. items only ever actually occupy the middle `0.75 * (2 * radius)` of the `2*radius`-square
// sphere element it creates, not the whole thing (confirmed by reading its source, not guessed).
// 0.45 (this component's own first version) left a real, live-reported gap between where tags
// visibly cluster and the container's own edge on every side. 0.6 accounts for that built-in
// three-quarters factor so the tags' own real visible spread — not just the invisible sphere
// element's box — reaches close to the container's edge (`0.6 * 0.75 * 2 ≈ 0.9`, i.e. ~90% of the
// container's own short side); the sphere element itself now runs slightly past the container's
// bounds on its long side, harmlessly clipped by the container's own `overflow: hidden`.
const RADIUS_RATIO = 0.6
// Below this many rendered tags, the sphere starts shrinking rather than staying at its full
// container-filling size — a small tag set (a fresh/near-empty library) spread across the same
// ~90%-of-container spread `RADIUS_RATIO` targets for a full 150-tag set left a large, obviously
// sparse sphere floating in mostly-empty space (live-reported after mocking a 5-tag set). Chosen
// as a fraction of `MAX_TAGS` rather than a fixed count so it scales if that cap ever changes.
const FULL_SIZE_TAG_COUNT = Math.round(MAX_TAGS * 0.2)
// The shrink floor — below `FULL_SIZE_TAG_COUNT` tags, radius scales down to this fraction of the
// full-size radius (never all the way to 0): a library with only 1-2 tags still gets a
// small-but-legible sphere instead of a barely-visible dot, and `levelFor`'s own font-size range
// still needs enough room to not visually collide even at the smallest tag counts.
const MIN_SIZE_RATIO = 0.3

/** Scales `radius` down for a small `tagCount`, from `MIN_SIZE_RATIO` (at 1 tag) up to 1.0 (at
 * `FULL_SIZE_TAG_COUNT` tags or more) — exported so `tests/unit/tagCloud.test.ts` can cover the
 * curve directly without a real DOM/`ResizeObserver` environment. A squared curve (not linear) —
 * a first, linear version was reported live as still visually "too big" around 5 tags, because a
 * straight line reaches roughly the *midpoint* ratio well before the low end of the count scale.
 * Squaring `t` keeps the ratio pinned near `MIN_SIZE_RATIO` for genuinely small counts and only
 * ramps up quickly as `tagCount` approaches `FULL_SIZE_TAG_COUNT`, so a handful of tags actually
 * reads as small instead of "already halfway to full size". */
export function sphereSizeRatio(tagCount: number): number {
  if (tagCount <= 0) return MIN_SIZE_RATIO
  if (tagCount >= FULL_SIZE_TAG_COUNT) return 1
  const t = tagCount / FULL_SIZE_TAG_COUNT
  return MIN_SIZE_RATIO + (1 - MIN_SIZE_RATIO) * t * t
}

// Exported (not just module-private) so tests/unit/tagCloud.test.ts can cover the actual
// weight→level mapping and HTML-escaping logic directly, without needing a real DOM/RAF/
// ResizeObserver environment the rest of this component depends on — those integration-level
// concerns aren't worth simulating in jsdom just to re-prove a third-party library's own 3D
// rendering works, but this app's own math/escaping logic is.
export function levelFor(weight: number, min: number, max: number): number {
  if (max === min) return Math.floor(STEPS / 2)
  return Math.round(((weight - min) * (STEPS - 1)) / (max - min)) + 1
}

/** Escapes the four characters that would otherwise break out of the `innerHTML` this library
 * assigns each item's text to verbatim (`useHTML: true` — see below) — a tag's own `text` is
 * archive-supplied data, not a trusted literal, so this must run before it's ever concatenated
 * into an HTML string. */
export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
}

export function TagCloud({ tags, onTagClick }: { tags: StatTag[]; onTagClick?: (tag: StatTag) => void }) {
  const { t } = useTranslation()
  const containerRef = useRef<HTMLDivElement>(null)
  const instanceRef = useRef<TagCloudInstance | null>(null)
  const pausedByVisibilityRef = useRef(false)
  const emptyMessage = t("stats.noTagsToShow") ?? "No tags to show yet."
  // Ref, not a `useLayoutEffect` dependency — `onTagClick` is read inside `build()`'s own
  // delegated click listener at call time (see below), so a caller passing a fresh inline
  // function on every render doesn't need to rebuild the whole sphere just to keep the handler
  // current.
  const onTagClickRef = useRef(onTagClick)
  onTagClickRef.current = onTagClick

  // Ranked by weight first, *then* capped — the tags that get dropped when the set is large are
  // always the least-weighted ones, never an arbitrary/first-N-in-API-order slice.
  const sortedDesc = [...tags].sort((a, b) => b.weight - a.weight)
  const rendered = sortedDesc.slice(0, MAX_TAGS)
  const weights = rendered.map((t) => t.weight)
  const maxWeight = weights[0] ?? 0
  const minWeight = weights[weights.length - 1] ?? 0

  function build() {
    const container = containerRef.current
    if (!container) return

    // A fresh child element every rebuild (mount, resize, tag-set change) rather than reusing one
    // across `destroy()` calls — see this file's own top docs on why `destroy()`'s real RAF-leak
    // makes "never truly reuse the old element" the actual mitigation, not just tidiness.
    container.replaceChildren()
    if (rendered.length === 0) {
      instanceRef.current = null
      // Graceful empty-state (issue #89's own "tag count is small or zero" requirement) — a bare
      // empty sphere-less container would otherwise just look broken, not "no data yet".
      const empty = document.createElement("div")
      empty.textContent = emptyMessage
      empty.style.opacity = "0.65"
      container.appendChild(empty)
      return
    }
    const sphereEl = document.createElement("div")
    container.appendChild(sphereEl)

    // Radius derived from the real container box (not a fixed constant) so the sphere fills
    // whatever space `Stats.tsx` gives this component — matches the old 2D port's own
    // `ResizeObserver`-driven "reflow on container size change" behavior (sidebar toggle, window
    // resize), just rebuilding the whole instance instead of re-running a placement algorithm.
    // `sphereSizeRatio` additionally shrinks it for a small `rendered.length` (see that function's
    // own docs) — `Math.max(80, ...)` still applies afterward as an absolute floor so this never
    // collapses to an illegibly tiny sphere even for a 1-tag library.
    const shortSide = Math.min(container.clientWidth, container.clientHeight)
    const radius = Math.max(80, Math.round(shortSide * RADIUS_RATIO * sphereSizeRatio(rendered.length)))

    const texts = rendered.map((tag) => {
      const level = levelFor(tag.weight, minWeight, maxWeight)
      const safeText = escapeHtml(tag.text)
      // Outer: the library's own per-frame `translate3d(...) scale(...)` target (untouched by
      // hover CSS). Inner: real hover-scale + transition, and `--tag-cloud-level` set to the real
      // per-theme color custom property this level maps to (see top docs) — read by
      // `.tag-cloud-3d-inner`'s own `color: var(--tag-cloud-level)` rule in `index.css`.
      return (
        "<span class=\"tag-cloud-3d-inner\" " +
        `style="font-size:${FONT_SIZE_PERCENT[level - 1]}%;` +
        `--tag-cloud-level:var(--tag-cloud-level-${level})">${safeText}</span>`
      )
    })

    const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    const instance = TagCloudLib(sphereEl, texts, {
      radius,
      // `keep: false` under reduced-motion — the sphere still responds to direct mouse/touch drag
      // (the accessibility target is "no *involuntary* motion", not "no interactivity at all"),
      // but decelerates back to a stop instead of drifting forever once the pointer leaves,
      // matching this issue's own "static 3D or manual-interaction-only" reduced-motion
      // requirement without a second, entirely separate non-animated rendering path to maintain.
      keep: !prefersReducedMotion,
      initSpeed: prefersReducedMotion ? "slow" : "normal",
      containerClass: "tag-cloud-3d",
      itemClass: "tag-cloud-3d-item",
      useHTML: true,
    })
    instanceRef.current = Array.isArray(instance) ? (instance[0] ?? null) : instance

    if (pausedByVisibilityRef.current) instanceRef.current?.pause()

    // Delegated (not per-item) — see this file's own top "Hovering a tag pauses" docs. `.closest`
    // rather than an exact `target === item` check since the real hovered element is the inner
    // `<span class="tag-cloud-3d-inner">` the mouse actually lands on, not its own parent item.
    sphereEl.addEventListener("mouseover", (e) => {
      if ((e.target as HTMLElement).closest(".tag-cloud-3d-item")) instanceRef.current?.pause()
    })
    sphereEl.addEventListener("mouseout", (e) => {
      const related = (e as MouseEvent).relatedTarget as Node | null
      // Only resume once the pointer has genuinely left the hovered item (not just moved from the
      // inner span to the outer item element within the same tag, which also fires `mouseout`) —
      // `relatedTarget` is where the pointer is going *to*; if that's still inside the same
      // `.tag-cloud-3d-item`, this isn't a real "left the tag" event yet.
      if (related instanceof Element && related.closest(".tag-cloud-3d-item")) return
      if (!pausedByVisibilityRef.current) instanceRef.current?.resume()
    })

    // Also delegated — a click maps back to the real `StatTag` it came from via DOM position. Note
    // `sphereEl` (the element this listener is attached to, and the one this component itself
    // creates) is *not* the item elements' direct parent — `TagCloudLib`'s own constructor takes
    // `sphereEl` as its `container` and creates one more wrapper `<div class="tag-cloud-3d">`
    // *inside* it, appending every item to that inner wrapper, not to `sphereEl` itself (confirmed
    // by reading its source: `_createElment` builds `$el`, appends each item to `$el`, then does
    // `self.$container.appendChild($el)` where `$container` is the element passed to the
    // constructor). So the index lookup must walk from `itemEl.parentElement` (that inner wrapper),
    // not from `sphereEl` — using `sphereEl.children` here always missed (`indexOf` → -1) since
    // `itemEl` was never actually a direct child of `sphereEl`. The order itself is still reliable:
    // `_createElment`'s own `texts.forEach` appends each item in `texts`/`rendered` order.
    sphereEl.addEventListener("click", (e) => {
      const itemEl = (e.target as HTMLElement).closest(".tag-cloud-3d-item")
      if (!itemEl?.parentElement) return
      const index = Array.from(itemEl.parentElement.children).indexOf(itemEl)
      const tag = rendered[index]
      if (tag) onTagClickRef.current?.(tag)
    })
  }

  useLayoutEffect(() => {
    build()
    const container = containerRef.current
    if (!container) return
    const observer = new ResizeObserver(() => build())
    observer.observe(container)
    return () => observer.disconnect()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tags])

  // Pauses the sphere's own continuous drift while the tab isn't visible — the library has no
  // `visibilitychange` awareness of its own, and an off-screen tab still burning a
  // `requestAnimationFrame` loop for pure visual drift nobody can see is the exact "wasted
  // background work" case this issue's own performance requirement calls out.
  useEffect(() => {
    function onVisibilityChange() {
      pausedByVisibilityRef.current = document.hidden
      if (document.hidden) instanceRef.current?.pause()
      else instanceRef.current?.resume()
    }
    document.addEventListener("visibilitychange", onVisibilityChange)
    return () => document.removeEventListener("visibilitychange", onVisibilityChange)
  }, [])

  return (
    <div
      ref={containerRef}
      className="tag-cloud-3d-container"
      style={{
        position: "relative",
        width: "100%",
        height: "100%",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        overflow: "hidden",
        // Base font every item's `100%`-`550%` size is relative to — not the page's ambient
        // font-size (a fixed, deliberate base, not a legacy-parity requirement now that this no
        // longer shares legacy's own `.jqcloud` class).
        font: "10px Helvetica, Arial, sans-serif",
        lineHeight: "normal",
      }}
    />
  )
}
