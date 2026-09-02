import { useEffect, useLayoutEffect, useRef } from "react"
import { useTranslation } from "react-i18next"
import TagCloudLib, { type TagCloud as TagCloudInstance } from "TagCloud"

import type { StatTag } from "@/api/types"

/** Dynamic 3D tag-cloud sphere built on `TagCloud@2.5.0`; builds the word list and layers in
 * pause/resume, hover, wheel-zoom, and touch drag on top of the library's own instance. */

const STEPS = 10
const FONT_SIZE_PERCENT = [100, 150, 200, 250, 300, 350, 400, 450, 500, 550]
const MAX_TAGS = 150
// Compensates for the library's own layout spreading items across only the middle 75% of its box.
const RADIUS_RATIO = 0.6
const FULL_SIZE_TAG_COUNT = Math.round(MAX_TAGS * 0.2)
const MIN_SIZE_RATIO = 0.3
const MIN_ZOOM = 0.5
const MAX_ZOOM = 2.5
// Below this container width, rendered tag count and font size shrink to avoid overlap on phones.
const DENSITY_FULL_WIDTH_PX = 480
const DENSITY_MIN_SCALE = 0.4
const FONT_SIZE_SCALE_FLOOR = 0.7

/** How much to shrink the rendered tag count for a small container. Exported for direct unit-test
 * coverage of the curve. */
export function densityScale(containerShortSidePx: number): number {
  if (containerShortSidePx >= DENSITY_FULL_WIDTH_PX) return 1
  if (containerShortSidePx <= 0) return DENSITY_MIN_SCALE
  const t = containerShortSidePx / DENSITY_FULL_WIDTH_PX
  return DENSITY_MIN_SCALE + (1 - DENSITY_MIN_SCALE) * t
}

/** Companion to `densityScale` — shrinks the base font size for the same small-container case. */
export function fontSizeScale(containerShortSidePx: number): number {
  if (containerShortSidePx >= DENSITY_FULL_WIDTH_PX) return 1
  if (containerShortSidePx <= 0) return FONT_SIZE_SCALE_FLOOR
  const t = containerShortSidePx / DENSITY_FULL_WIDTH_PX
  return FONT_SIZE_SCALE_FLOOR + (1 - FONT_SIZE_SCALE_FLOOR) * t
}
// Normalizes wheel deltaY across devices so one "click" doesn't jump several zoom steps at once.
const ZOOM_SENSITIVITY = 1000

/** Scales `radius` down for a small `tagCount`, from `MIN_SIZE_RATIO` up to 1.0 at
 * `FULL_SIZE_TAG_COUNT`. Squared curve so small counts stay visibly small. */
export function sphereSizeRatio(tagCount: number): number {
  if (tagCount <= 0) return MIN_SIZE_RATIO
  if (tagCount >= FULL_SIZE_TAG_COUNT) return 1
  const t = tagCount / FULL_SIZE_TAG_COUNT
  return MIN_SIZE_RATIO + (1 - MIN_SIZE_RATIO) * t * t
}

// Exported so unit tests can cover weight→level mapping and escaping without a real DOM/RAF env.
export function levelFor(weight: number, min: number, max: number): number {
  if (max === min) return Math.floor(STEPS / 2)
  return Math.round(((weight - min) * (STEPS - 1)) / (max - min)) + 1
}

/** Escapes HTML special chars before a tag's text is concatenated into the `innerHTML` string this
 * library assigns per item (`useHTML: true`). */
export function escapeHtml(s: string): string {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;")
}

export function TagCloud({ tags, onTagClick }: { tags: StatTag[]; onTagClick?: (tag: StatTag) => void }) {
  const { t } = useTranslation()
  const containerRef = useRef<HTMLDivElement>(null)
  const instanceRef = useRef<TagCloudInstance | null>(null)
  const pausedByVisibilityRef = useRef(false)
  const emptyMessage = t("stats.noTagsToShow") ?? "No tags to show yet."
  const onTagClickRef = useRef(onTagClick)
  onTagClickRef.current = onTagClick
  // zoomRef persists zoom across rebuilds so a resize/tag-set change doesn't reset a visitor's zoom.
  const zoomRef = useRef(1)

  const sortedDesc = [...tags].sort((a, b) => b.weight - a.weight).slice(0, MAX_TAGS)

  function build() {
    const container = containerRef.current
    if (!container) return

    container.replaceChildren()

    const shortSide = Math.min(container.clientWidth, container.clientHeight)
    const effectiveMaxTags = Math.max(1, Math.round(MAX_TAGS * densityScale(shortSide)))
    const rendered = sortedDesc.slice(0, effectiveMaxTags)
    const weights = rendered.map((tag) => tag.weight)
    const maxWeight = weights[0] ?? 0
    const minWeight = weights[weights.length - 1] ?? 0

    if (rendered.length === 0) {
      instanceRef.current = null
      const empty = document.createElement("div")
      empty.textContent = emptyMessage
      empty.style.opacity = "0.65"
      container.appendChild(empty)
      return
    }
    const sphereEl = document.createElement("div")
    container.appendChild(sphereEl)
    sphereEl.style.transformOrigin = "center"
    sphereEl.style.transform = `scale(${zoomRef.current})`

    const radius = Math.max(80, Math.round(shortSide * RADIUS_RATIO * sphereSizeRatio(rendered.length)))
    container.style.fontSize = `${(10 * fontSizeScale(shortSide)).toFixed(2)}px`

    const texts = rendered.map((tag) => {
      const level = levelFor(tag.weight, minWeight, maxWeight)
      const safeText = escapeHtml(tag.text)
      return (
        "<span class=\"tag-cloud-3d-inner\" " +
        `style="font-size:${FONT_SIZE_PERCENT[level - 1]}%;` +
        `--tag-cloud-level:var(--tag-cloud-level-${level})">${safeText}</span>`
      )
    })

    const prefersReducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    const instance = TagCloudLib(sphereEl, texts, {
      radius,
      containerClass: "tag-cloud-3d",
      itemClass: "tag-cloud-3d-item",
      useHTML: true,
    })
    instanceRef.current = Array.isArray(instance) ? (instance[0] ?? null) : instance

    if (prefersReducedMotion) instanceRef.current?.pause()
    if (pausedByVisibilityRef.current) instanceRef.current?.pause()

    sphereEl.addEventListener("mouseover", (e) => {
      if ((e.target as HTMLElement).closest(".tag-cloud-3d-item")) instanceRef.current?.pause()
    })
    sphereEl.addEventListener("mouseout", (e) => {
      const related = (e as MouseEvent).relatedTarget as Node | null
      if (related instanceof Element && related.closest(".tag-cloud-3d-item")) return
      if (!pausedByVisibilityRef.current && !prefersReducedMotion) instanceRef.current?.resume()
    })

    sphereEl.addEventListener("click", (e) => {
      const itemEl = (e.target as HTMLElement).closest(".tag-cloud-3d-item")
      if (!itemEl?.parentElement) return
      const index = Array.from(itemEl.parentElement.children).indexOf(itemEl)
      const tag = rendered[index]
      if (tag) onTagClickRef.current?.(tag)
    })

    // Writes mouseX/mouseY/active directly since the library never attaches its own touch listener.
    const rawInstance = instanceRef.current as unknown as { mouseX: number; mouseY: number; active: boolean } | null
    function applyTouchAsPointer(touch: Touch) {
      if (!rawInstance) return
      const rect = sphereEl.getBoundingClientRect()
      rawInstance.mouseX = (touch.clientX - (rect.left + rect.width / 2)) / 5
      rawInstance.mouseY = (touch.clientY - (rect.top + rect.height / 2)) / 5
    }
    sphereEl.addEventListener(
      "touchstart",
      (e) => {
        if (!rawInstance) return
        rawInstance.active = true
        if (prefersReducedMotion) instanceRef.current?.resume()
        const touch = e.touches[0]
        if (touch) applyTouchAsPointer(touch)
      },
      { passive: true },
    )
    sphereEl.addEventListener(
      "touchmove",
      (e) => {
        const touch = e.touches[0]
        if (touch) applyTouchAsPointer(touch)
      },
      { passive: true },
    )
    sphereEl.addEventListener(
      "touchend",
      () => {
        if (rawInstance) rawInstance.active = false
        if (prefersReducedMotion) instanceRef.current?.pause()
      },
      { passive: true },
    )
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

  useEffect(() => {
    function onVisibilityChange() {
      pausedByVisibilityRef.current = document.hidden
      if (document.hidden) instanceRef.current?.pause()
      else instanceRef.current?.resume()
    }
    document.addEventListener("visibilitychange", onVisibilityChange)
    return () => document.removeEventListener("visibilitychange", onVisibilityChange)
  }, [])

  useEffect(() => {
    const container = containerRef.current
    if (!container) return
    function onWheel(e: WheelEvent) {
      const sphereEl = container?.querySelector<HTMLElement>(".tag-cloud-3d")
      if (!sphereEl) return
      e.preventDefault()
      const next = zoomRef.current - e.deltaY / ZOOM_SENSITIVITY
      zoomRef.current = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, next))
      sphereEl.style.transform = `scale(${zoomRef.current})`
    }
    container.addEventListener("wheel", onWheel, { passive: false })
    return () => container.removeEventListener("wheel", onWheel)
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
        font: "10px Helvetica, Arial, sans-serif",
        lineHeight: "normal",
      }}
    />
  )
}
