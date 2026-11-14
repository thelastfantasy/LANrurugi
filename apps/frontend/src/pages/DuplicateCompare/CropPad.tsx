import { useEffect } from "react";

import type { ComparisonSide, CropAlignment } from "@/api/types";

/** Fraction (0..1) of this side's own frame, per edge, that falls outside the region matched to
 * the other side. All zero for an identity alignment. */
export interface EdgeBands {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

/** Solves the `scale`+`offset` affine map for the sub-range of `side`'s frame with a
 * corresponding point in the other side's frame; the complement is content unique to `side`. */
export function computeEdgeBands(side: ComparisonSide, alignment: CropAlignment): EdgeBands {
  const { scale, offset_x, offset_y } = alignment;
  let loX: number, hiX: number, loY: number, hiY: number;
  if (side === "a") {
    loX = clamp01((0 - offset_x) / scale);
    hiX = clamp01((1 - offset_x) / scale);
    loY = clamp01((0 - offset_y) / scale);
    hiY = clamp01((1 - offset_y) / scale);
  } else {
    loX = clamp01(offset_x);
    hiX = clamp01(offset_x + scale);
    loY = clamp01(offset_y);
    hiY = clamp01(offset_y + scale);
  }
  return {
    left: Math.max(0, loX),
    right: Math.max(0, 1 - hiX),
    top: Math.max(0, loY),
    bottom: Math.max(0, 1 - hiY),
  };
}

/** Whether `side`'s real pixel count is smaller than `otherWidth`×`otherHeight` — the single
 * source of truth for "does this side get the synthetic pad," shared with `MagnifierOverlay`. */
export function needsSyntheticPad(
  alignment: CropAlignment,
  sideWidth: number,
  sideHeight: number,
  otherWidth: number,
  otherHeight: number,
): boolean {
  const isIdentity = alignment.scale === 1 && alignment.offset_x === 0 && alignment.offset_y === 0;
  return !isIdentity && sideWidth * sideHeight > 0 && sideWidth * sideHeight < otherWidth * otherHeight;
}

/** Synthetic-pad geometry as fractions of the padded canvas buffer: `offsetX`/`offsetY` is where
 * real image content starts, `contentWidth`/`contentHeight` is how much it occupies. */
export interface PadGeometry {
  offsetX: number;
  offsetY: number;
  contentWidth: number;
  contentHeight: number;
}

/** Computes `PadGeometry` for `side` — callers should gate on `needsSyntheticPad` first. */
export function computePadGeometry(side: ComparisonSide, alignment: CropAlignment): PadGeometry {
  const otherSide: ComparisonSide = side === "a" ? "b" : "a";
  const otherBands = computeEdgeBands(otherSide, alignment);
  return {
    offsetX: otherBands.left,
    offsetY: otherBands.top,
    contentWidth: 1 - otherBands.left - otherBands.right,
    contentHeight: 1 - otherBands.top - otherBands.bottom,
  };
}

/** Fallback colors if `.ai-compare-stripe-source`'s computed styles can't be read. */
const FALLBACK_STRIPE_COLOR = "rgba(230, 126, 34, 0.55)";
const FALLBACK_BACKDROP_COLOR = "#0a0a0a";

/** Reads the theme's stripe colors off `canvas`'s computed `.ai-compare-stripe-source` CSS class —
 * `background-color` for the stripe, `border-color` (repurposed) for the backdrop. */
function readStripeColors(canvas: HTMLCanvasElement): { stripe: string; backdrop: string } {
  const computed = getComputedStyle(canvas);
  return {
    stripe: computed.backgroundColor || FALLBACK_STRIPE_COLOR,
    backdrop: computed.borderColor || FALLBACK_BACKDROP_COLOR,
  };
}

/** Fills `rect` with a 45°-diagonal stripe pattern; stripe width scales with `rect`'s shorter
 * dimension so density stays consistent across resolutions, capped at `MAX_STRIPE_WIDTH_PX`. */
const MAX_STRIPE_WIDTH_PX = 12;
/** Gap between stripes as a multiple of stripe width. */
const STRIPE_GAP_RATIO = 2.5;

function drawStripedRect(
  ctx: CanvasRenderingContext2D,
  canvas: HTMLCanvasElement,
  rect: { left: number; top: number; width: number; height: number },
) {
  if (rect.width <= 0 || rect.height <= 0) return;
  const stripeWidth = Math.min(
    MAX_STRIPE_WIDTH_PX,
    Math.max(2, Math.round(Math.min(rect.width, rect.height) * 0.012)),
  );
  const step = stripeWidth * (1 + STRIPE_GAP_RATIO);
  const { stripe, backdrop } = readStripeColors(canvas);
  ctx.save();
  ctx.beginPath();
  ctx.rect(rect.left, rect.top, rect.width, rect.height);
  ctx.clip();
  ctx.fillStyle = backdrop;
  ctx.fillRect(rect.left, rect.top, rect.width, rect.height);
  ctx.fillStyle = stripe;
  const overshoot = rect.width + rect.height + stripeWidth;
  const diagonal = rect.width + rect.height;
  // Perpendicular offset for a `stripeWidth`-thick band centered on a 45° line: stripeWidth/2/√2.
  const perp = stripeWidth / 2 / Math.SQRT2;
  for (let offset = -diagonal - overshoot; offset < diagonal + overshoot; offset += step) {
    const x0 = rect.left + offset;
    const y0 = rect.top - overshoot;
    const x1 = x0 + rect.height + 2 * overshoot;
    const y1 = rect.top + rect.height + overshoot;
    ctx.beginPath();
    ctx.moveTo(x0 - perp, y0 + perp);
    ctx.lineTo(x0 + perp, y0 - perp);
    ctx.lineTo(x1 + perp, y1 - perp);
    ctx.lineTo(x1 - perp, y1 + perp);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();
}

/** Draws the border-free side's synthetic pad onto a canvas sized to the other side's real
 * resolution, so both sides' content aligns. Pure side-effect component — no JSX of its own. */
export function AlignmentBandOverlay({
  preloadImgRef,
  canvasRef,
  paddedSide,
  alignment,
  needsPad,
  otherWidth,
  otherHeight,
}: {
  /** Dedicated `<img>` with a fixed `src`, independent of the toggle-driven visible `<img>` — lets
   * decoding start as soon as the modal opens rather than on first toggle. */
  preloadImgRef: React.RefObject<HTMLImageElement | null>;
  /** Owned by `OverlayPage`, not this component — `MagnifierOverlay` samples its zoomed lens from
   * this same canvas element when `needsPad` is true. */
  canvasRef: React.RefObject<HTMLCanvasElement | null>;
  /** Which side (`"a"` or `"b"`) this canvas represents — fixed per sample. */
  paddedSide: ComparisonSide;
  alignment: CropAlignment;
  /** Whether `paddedSide` needs the synthetic pad — computed once by the caller via `needsSyntheticPad`. */
  needsPad: boolean;
  /** The other side's real pixel dimensions — the canvas buffer's own resolution. */
  otherWidth: number;
  otherHeight: number;
}) {
  useEffect(() => {
    const pad = computePadGeometry(paddedSide, alignment);

    function draw() {
      const canvas = canvasRef.current;
      const img = preloadImgRef.current;
      if (!canvas) return;
      const ctx = canvas.getContext("2d");
      if (!ctx) return;

      if (!needsPad || !img || img.naturalWidth === 0 || otherWidth <= 0 || otherHeight <= 0) {
        canvas.width = 0;
        canvas.height = 0;
        return;
      }

      canvas.width = otherWidth;
      canvas.height = otherHeight;
      ctx.clearRect(0, 0, otherWidth, otherHeight);

      // Stripe the entire buffer, then draw the image on top — whatever it doesn't cover is the border.
      drawStripedRect(ctx, canvas, { left: 0, top: 0, width: otherWidth, height: otherHeight });
      ctx.drawImage(img, pad.offsetX * otherWidth, pad.offsetY * otherHeight);
    }

    draw();
    const img = preloadImgRef.current;
    img?.addEventListener("load", draw);
    return () => img?.removeEventListener("load", draw);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [paddedSide, alignment.scale, alignment.offset_x, alignment.offset_y, needsPad, otherWidth, otherHeight]);

  return null;
}
