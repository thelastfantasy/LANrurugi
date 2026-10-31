// Client-side compositing for the locally-hosted-backend path
// (`contracts/client-compositing-cache.md`, research.md §6/§7).
//
// Plain Canvas 2D — no WASM. The operation here is "draw text with a background box over an image",
// which Canvas already does natively; WASM would only earn its complexity for something genuinely
// heavy (e.g. inpainting to remove the original lettering), which is explicitly out of scope this
// phase — FR-005 requires translated text *positioned over* the original region, not the original
// removed.
//
// This runs in the browser rather than on the server because, for a locally-hosted backend, the
// translated text only ever exists here (constitution Principle V): the server can't reach the
// user's loopback model, so shipping the text back purely to be drawn would be a pointless round
// trip.

import type { BoundingBox, DetectedTextRegion, Rgb } from "./types"

/** Fallbacks for attributes the server's heuristic couldn't estimate (FR-008a). Each applies
 * independently — a missing boldness must never suppress a known colour, or vice versa. */
const DEFAULT_FG = "#141414"
const DEFAULT_BG = "#fafafa"

const MIN_FONT_PX = 9
const MAX_FONT_PX = 72
/** Fraction of the region's width kept clear on each side. */
const HORIZONTAL_PADDING = 0.04
const LINE_HEIGHT_RATIO = 1.2

function toCss(color: Rgb | undefined, fallback: string): string {
  if (!color) return fallback
  return `rgb(${color.r}, ${color.g}, ${color.b})`
}

/**
 * Maps a golden-set font id to a real CSS font stack.
 *
 * The golden set holds lettering *styles* rather than specific typeface files (the server has no
 * licensed manga font to name), so each maps to a generic family the browser can actually resolve.
 */
export function cssFontFamily(fontId: string | undefined): string {
  switch (fontId) {
    case "sfx-brush":
      return '"Comic Sans MS", "Chalkboard SE", cursive, sans-serif'
    case "narration-mincho":
      return 'Georgia, "Times New Roman", serif'
    case "dialogue-bold":
    case "dialogue-gothic":
    default:
      return '"Helvetica Neue", Arial, sans-serif'
  }
}

/** Greedy word wrap against a measured canvas context. */
function wrapText(ctx: CanvasRenderingContext2D, text: string, maxWidth: number): string[] {
  const lines: string[] = []
  let current = ""

  for (const word of text.split(/\s+/).filter(Boolean)) {
    const candidate = current ? `${current} ${word}` : word
    if (ctx.measureText(candidate).width <= maxWidth) {
      current = candidate
      continue
    }
    if (current) lines.push(current)

    if (ctx.measureText(word).width <= maxWidth) {
      current = word
    } else {
      // A single word wider than the line: break it character by character.
      let chunk = ""
      for (const ch of word) {
        if (ctx.measureText(chunk + ch).width > maxWidth && chunk) {
          lines.push(chunk)
          chunk = ""
        }
        chunk += ch
      }
      current = chunk
    }
  }

  if (current) lines.push(current)
  return lines.length > 0 ? lines : [""]
}

/**
 * Finds the largest font size at which the text fits the region.
 *
 * Shrink-to-fit rather than overflow: text spilling outside its bubble would cover neighbouring
 * artwork, which is worse than being slightly smaller.
 */
function fitText(
  ctx: CanvasRenderingContext2D,
  text: string,
  box: BoundingBox,
  fontFamily: string,
  bold: boolean,
): { fontSize: number; lines: string[] } {
  const usableWidth = box.w * (1 - 2 * HORIZONTAL_PADDING)
  const weight = bold ? "700" : "400"

  for (
    let size = Math.min(MAX_FONT_PX, Math.max(MIN_FONT_PX, Math.floor(box.h * 0.5)));
    size >= MIN_FONT_PX;
    size -= 1
  ) {
    ctx.font = `${weight} ${size}px ${fontFamily}`
    const lines = wrapText(ctx, text, usableWidth)
    if (lines.length * size * LINE_HEIGHT_RATIO <= box.h) {
      return { fontSize: size, lines }
    }
  }

  ctx.font = `${weight} ${MIN_FONT_PX}px ${fontFamily}`
  return { fontSize: MIN_FONT_PX, lines: wrapText(ctx, text, usableWidth) }
}

/** Draws one region's translated text over its original area. */
function drawRegion(ctx: CanvasRenderingContext2D, region: DetectedTextRegion): void {
  const text = region.translatedText?.trim()
  if (!text) return

  const box = region.boundingBox
  if (box.w <= 0 || box.h <= 0) return

  // Each attribute falls back on its own (FR-008a).
  const fg = toCss(region.fgColor, DEFAULT_FG)
  const bg = toCss(region.bgColor, DEFAULT_BG)
  const bold = region.isBold ?? false
  const fontFamily = cssFontFamily(region.font)

  // Cover the original lettering so the translation is legible on top of it.
  ctx.fillStyle = bg
  ctx.fillRect(box.x, box.y, box.w, box.h)

  const { fontSize, lines } = fitText(ctx, text, box, fontFamily, bold)
  const lineHeight = fontSize * LINE_HEIGHT_RATIO

  ctx.fillStyle = fg
  ctx.textAlign = "center"
  ctx.textBaseline = "middle"

  const totalHeight = lines.length * lineHeight
  const centerX = box.x + box.w / 2
  let y = box.y + Math.max(0, (box.h - totalHeight) / 2) + lineHeight / 2

  for (const line of lines) {
    ctx.fillText(line, centerX, y)
    y += lineHeight
  }
}

/**
 * Composites translated regions over the original page image.
 *
 * Returns a PNG `Blob` suitable for the IndexedDB cache and for display. Regions without a
 * translation are skipped, so a partially-translated page still renders what it has (FR-020).
 *
 * LTR-only (research.md §11): no bidi handling or RTL-aware justification is attempted.
 */
export async function compositePage(
  pageImage: ImageBitmap | HTMLImageElement,
  regions: DetectedTextRegion[],
): Promise<Blob> {
  const width = pageImage.width
  const height = pageImage.height

  // `OffscreenCanvas` keeps the work off the main thread's layout path where available; the
  // regular canvas is a straightforward fallback, not a different code path for the drawing itself.
  if (typeof OffscreenCanvas !== "undefined") {
    const canvas = new OffscreenCanvas(width, height)
    const ctx = canvas.getContext("2d")
    if (!ctx) throw new Error("failed to acquire a 2D context")

    ctx.drawImage(pageImage, 0, 0)
    for (const region of regions) drawRegion(ctx as unknown as CanvasRenderingContext2D, region)
    return canvas.convertToBlob({ type: "image/png" })
  }

  const canvas = document.createElement("canvas")
  canvas.width = width
  canvas.height = height
  const ctx = canvas.getContext("2d")
  if (!ctx) throw new Error("failed to acquire a 2D context")

  ctx.drawImage(pageImage, 0, 0)
  for (const region of regions) drawRegion(ctx, region)

  return new Promise((resolve, reject) => {
    canvas.toBlob((blob) => {
      if (blob) resolve(blob)
      else reject(new Error("failed to encode the composited page"))
    }, "image/png")
  })
}
