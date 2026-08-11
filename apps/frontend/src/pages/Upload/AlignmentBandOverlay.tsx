import { useEffect } from "react";

import type { ComparisonSide, CropAlignment } from "@/api/types";

/** How much of THIS side's own frame, along each edge, falls outside the region matched to the
 * OTHER side — as a 0..1 fraction of this side's own width/height. All zero for an identity
 * alignment (`scale: 1, offset_x: 0, offset_y: 0`), or when nothing on this side's own edges is
 * unmatched (i.e. this side is the "content-full" one — see [`AlignmentBandOverlay`]'s own docs
 * for what happens to that side). */
export interface EdgeBands {
  top: number;
  bottom: number;
  left: number;
  right: number;
}

function clamp01(v: number): number {
  return Math.min(1, Math.max(0, v));
}

/** For `side === "a"`: solves `a*scale+offset ∈ [0,1]` for `a` (the sub-range of A's own frame
 * whose corresponding B point actually exists within B's frame) — the complement of that range,
 * per edge, is content unique to A. For `side === "b"`: the inverse, `b ∈ [offset, offset+scale]`
 * — the complement is content unique to B (typically an added scan border/frame, this feature's
 * own real motivating case — see `crop_align`'s own docs). Both reduce to the same shape since
 * the forward and inverse of a `scale`+`offset` affine map are both themselves affine. Assumes
 * `scale > 0`, which every real `CropAlignment` this app produces satisfies (`crop_align.rs`'s own
 * `SCALE_CANDIDATES` are all positive, and the identity fallback has `scale: 1`). */
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
  // A degenerate/inverted range (e.g. an extreme offset pushing the whole valid range out of
  // [0,1]) would make `hi < lo` — clamp each band width at 0 rather than letting it go negative.
  return {
    left: Math.max(0, loX),
    right: Math.max(0, 1 - hiX),
    top: Math.max(0, loY),
    bottom: Math.max(0, 1 - hiY),
  };
}

/** Whether `side`'s own real pixel count is smaller than `otherWidth`×`otherHeight` — the single
 * source of truth for "does this side get the synthetic pad," shared by [`AlignmentBandOverlay`]
 * (which draws it) and `MagnifierOverlay`'s own pointer math (which needs to know whether to
 * treat clicks/hovers against the padded canvas's own coordinate space instead of the plain
 * `<img>`'s native `object-fit: contain` layout — see [`computePadGeometry`]'s own docs). Takes
 * `sideWidth`/`sideHeight` directly (from `sample.a_width`/`a_height` etc, already known
 * synchronously from the comparison result) rather than a live `<img>` ref's `naturalWidth` —
 * unlike `AlignmentBandOverlay`'s own canvas draw (which genuinely needs the loaded `<img>` to
 * call `drawImage` on), the yes/no PAD DECISION doesn't need to wait for image load at all, and
 * computing it from `sample` data keeps `OverlayPage`'s magnifier wiring and
 * `AlignmentBandOverlay`'s own draw effect using the exact same decision with no risk of a
 * load-timing race disagreeing between them. */
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

/** The synthetic-pad geometry [`AlignmentBandOverlay`] draws for a side — as fractions of the
 * padded canvas's own buffer (== the reference/other side's real resolution): `offsetX`/`offsetY`
 * is where the real image content starts (top-left), `contentWidth`/`contentHeight` is how much
 * of the buffer that real content occupies. `MagnifierOverlay`'s own pointer math consumes this
 * shape too (via [`computePadGeometry`]) so a click/hover against the padded canvas maps to the
 * correct real-image UV point instead of the plain `<img>`'s own (no-longer-visually-accurate,
 * once padding is active) `object-fit: contain` centering. */
export interface PadGeometry {
  offsetX: number;
  offsetY: number;
  contentWidth: number;
  contentHeight: number;
}

/** Computes [`PadGeometry`] for `side` — callers should gate on [`needsSyntheticPad`] first; this
 * function doesn't check that itself (it just describes the geometry IF padding is active, same
 * split of concerns as `AlignmentBandOverlay`'s own draw effect keeping the yes/no decision and
 * the geometry computation separate). */
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

/** Fallback colors used only if `.ai-compare-stripe-source`'s own computed styles can't be read
 * (e.g. the class rule failed to load) — should never be visible in practice, since every real
 * theme file defines that class (see [`readStripeColors`]'s own docs). */
const FALLBACK_STRIPE_COLOR = "rgba(230, 126, 34, 0.55)";
const FALLBACK_BACKDROP_COLOR = "#0a0a0a";

/** Reads the stripe pattern's own two colors as PLAIN STRINGS off `canvas`'s computed CSS —
 * `background-color` for the stripe itself, `border-color` (arbitrarily repurposed; a `<canvas>`
 * has no meaningful border of its own to conflict with) for the backdrop between stripes.
 *
 * IMPORTANT: this is not "CSS styles the canvas" — `background-color`/`border-color` set via CSS
 * on a `<canvas>` element only affect that element's own HTML BOX (and are invisible here anyway,
 * fully covered by the canvas's own drawn bitmap). What actually happens: `.ai-compare-stripe-
 * source` (declared per-theme in each of the 5 real theme files under
 * `apps/frontend/public/legacy/themes/`) is used purely as a THEME-COLOR LOOKUP TABLE — CSS is the
 * one place this project already has a working "5 different values depending on which theme is
 * active" mechanism (see this project's own custom-color rule: hardcoding one color for every
 * theme is exactly what that rule forbids). `getComputedStyle(canvas)` resolves that class's rule
 * for whichever theme is actually active right now and hands back a plain color string, which is
 * then assigned to `ctx.fillStyle` below — an entirely separate, manual bitmap draw call, nothing
 * to do with the canvas element's own CSS box rendering. `canvas` carries the class (see
 * `OverlayPage.tsx`'s own `className` on it) purely so this function has a real, already-in-the-
 * DOM element to resolve computed styles against, instead of needing a separate hidden marker
 * node just for this lookup. */
function readStripeColors(canvas: HTMLCanvasElement): { stripe: string; backdrop: string } {
  const computed = getComputedStyle(canvas);
  return {
    stripe: computed.backgroundColor || FALLBACK_STRIPE_COLOR,
    backdrop: computed.borderColor || FALLBACK_BACKDROP_COLOR,
  };
}

/** Fills `rect` (in canvas buffer pixels) with a 45°-diagonal stripe pattern, stripe width/gap
 * scaled to `rect`'s own shorter dimension so the pattern reads at a consistent visual density
 * regardless of the buffer's real resolution (a raw pixel constant would look vanishingly thin
 * against a several-thousand-pixel-wide scan) — capped at [`MAX_STRIPE_WIDTH_PX`] so a very large
 * scan doesn't scale the stripes up unboundedly. Went through two rounds of live width/density
 * tuning against a real ~1700px-tall sample: the original uncapped 5% fraction with a 1:1
 * stripe:gap ratio read as "太粗了" (too thick); the first fix (2.5% fraction, still 1:1) still
 * read as "太粗太密" (still too thick AND too dense) once actually seen against real page content
 * — the gap needed to widen relative to the stripe, not just the stripe itself shrink.
 *
 * Each stripe is a FILLED parallelogram (`fill()`, not `stroke()`) whose own four corners sit well
 * OUTSIDE `rect` on every side (see `overshoot` below) — `ctx.clip()` then does the only trimming
 * that ever happens, at `rect`'s own straight edges. Two earlier attempts both left a visible
 * notch/corner artifact right where a stripe met `rect`'s own top or bottom edge: a stroked
 * diagonal line's butt-cap is perpendicular to the STROKE's own 45° direction, not to the
 * horizontal clip edge, so the cap sliced across it at an angle; a later attempt built each
 * parallelogram's corners to sit exactly AT the diagonal's own start/end (still inside/adjacent to
 * `rect`), so that polygon's own corner vertices were what poked into view. Neither is a fluke of
 * "diagonal stripes meeting a horizontal edge are inherently jagged" (confirmed live, user
 * correctly pushed back on that framing) — a diagonal stripe truncated by a straight clip edge IS
 * just a straight diagonal cut; the jaggedness both previous attempts showed came from each
 * stripe's OWN polygon geometry ending near the visible boundary, not from the clip itself. Making
 * every stripe extend well past `rect` on all four sides before `fill()` removes that ambiguity
 * entirely — the ONLY edges `ctx.clip()` ever has to cut across are `rect`'s own four straight
 * sides, so there is nothing left to produce a notch. */
const MAX_STRIPE_WIDTH_PX = 12;
/** Gap between stripes as a multiple of `stripeWidth` — 2.5, not 1 (an equal-width gap, the
 * original value), so the backdrop reads as the dominant surface with thin accent stripes over
 * it, not a dense, roughly-50%-coverage hazard pattern. */
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
  // How far each stripe's own polygon extends past `rect` on every side before `fill()` — larger
  // than `rect`'s own diagonal, so no stripe's real end (nor its perpendicular-offset corner) can
  // ever land inside or adjacent to the clipped/visible area regardless of `rect`'s own aspect
  // ratio.
  const overshoot = rect.width + rect.height + stripeWidth;
  const diagonal = rect.width + rect.height;
  // Perpendicular to a 45° line is also 45°, so a `stripeWidth`-thick band centered on that line
  // is offset by `stripeWidth / 2 / √2` along each axis (the perpendicular unit vector for a
  // (1,1)-direction line is (-1,1)/√2, and moving `stripeWidth/2` along it shifts x and y by that
  // amount each) — matches the same `stripeWidth` a `lineWidth`-thick stroke along this diagonal
  // would have covered, measured perpendicular to the line, not axis-aligned.
  const perp = stripeWidth / 2 / Math.SQRT2;
  for (let offset = -diagonal - overshoot; offset < diagonal + overshoot; offset += step) {
    const x0 = rect.left + offset;
    const y0 = rect.top - overshoot;
    const x1 = x0 + rect.height + 2 * overshoot;
    const y1 = rect.top + rect.height + overshoot;
    // A filled parallelogram tracing the same 45° band a `lineWidth`-thick stroke along
    // `(x0, y0)` → `(x1, y1)` would have covered, extended well past `rect` on every side (see
    // this function's own docs for why).
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

/** When one version has an added scan border/frame, its real content renders SMALLER on screen
 * than the other, border-free version's content once both are fit into equal-sized boxes via
 * `object-fit: contain` — the border eats into the same display budget the content would
 * otherwise fill. To make the two versions' CONTENT overlap/align on screen (not just both be
 * visible), the border-free side gets its own synthetic "border" here: drawn onto a canvas whose
 * own buffer resolution matches the OTHER (bordered) side's real pixel dimensions
 * (`otherWidth`/`otherHeight`), with this side's real image placed at its own native size,
 * offset by the other side's real border widths (converted from its own edge-band FRACTIONS to
 * real pixels via `otherWidth`/`otherHeight`) — deliberately NOT rescaled to fit some derived
 * "content size" first (confirmed live: the real border-adding case pastes the original image in
 * completely unscaled, so its own native resolution already IS the correct inner size; rescaling
 * on top of that would be redundant work and a needless quality hit).
 *
 * Draws from `preloadImgRef` — a DEDICATED `<img>` `OverlayPage` keeps mounted with a FIXED `src`
 * pointing at `paddedSide`'s own page (see that prop's own docs), independent of whichever side
 * the user currently has toggled to — NOT the toggle-driven visible `<img>` whose `src` swaps
 * between A/B. This is what lets the canvas finish decoding and drawing in the background as soon
 * as the modal opens, rather than only starting once the user's first Shift press makes
 * `paddedSide` the CURRENTLY DISPLAYED one — confirmed live via a captured performance trace: with
 * the old (toggle-driven-`<img>`-only) design, the first Shift press across even a single sample
 * page triggered a burst of `ImageDecodeTask`/`Decode Image` work for every sample's B-side image
 * decoding at once (they'd all been sitting un-decoded until that exact moment), a real, measured
 * ~70ms stall competing for the same decode threads — a background-mounted dedicated `<img>` per
 * padded sample spreads that same decode work out over however long the modal's been open before
 * the user's first interaction instead of concentrating it all into that one keypress.
 *
 * Resets the canvas to 0x0 (no drawn content) when `needsPad` is false — the caller decides that
 * via [`needsSyntheticPad`], not this component itself (see that function's own docs for why
 * comparing real pixel COUNT, not checking whether this side's own `computeEdgeBands` is exactly
 * zero, is the reliable way to tell "which side is the bordered one" for real, estimated alignment
 * data).
 *
 * A pure side-effect component — renders no JSX of its own. `OverlayPage` owns the actual
 * `<canvas ref={canvasRef}>` element (its box-defining CSS, and — while the user has toggled to
 * `paddedSide` — its real pointer-event handlers too, since that canvas IS the interactive surface
 * for that side, not a redundant `<img>` sitting invisibly underneath it — confirmed live: "既然
 * canvas本身就包含无白边图片，那么对应的img元素就不要放在DOM里了，多余"). This component just
 * keeps that canvas's CONTENT current, always, the moment `preloadImgRef` has real pixels —
 * independent of whether `OverlayPage` currently has it visible/interactive at all, which is what
 * lets the draw happen in the background before the user ever toggles to this side. Drawn at a
 * fixed native resolution; `OverlayPage`'s own `object-fit: contain` styling on the canvas element
 * handles all responsive resizing the same way it would for a normal `<img>` — no
 * `getBoundingClientRect`-driven redraw loop needed the way the first version of this component
 * had. */
export function AlignmentBandOverlay({
  preloadImgRef,
  canvasRef,
  paddedSide,
  alignment,
  needsPad,
  otherWidth,
  otherHeight,
}: {
  /** A `<img>` `OverlayPage` mounts with a FIXED `src` = `paddedSide`'s own page URL — see this
   * component's own docs for why a SEPARATE, toggle-independent element (not the visible,
   * toggle-driven `<img>`) is what lets decoding start as soon as the modal opens. */
  preloadImgRef: React.RefObject<HTMLImageElement | null>;
  /** Owned by `OverlayPage`, not this component — `MagnifierOverlay`'s own pointer math needs the
   * SAME canvas element (not a copy) to sample its zoomed lens from when `needsPad` is true (the
   * border-free side is never shown as a plain `<img>` once padding is active — confirmed live:
   * "这张没白边的图片根本不作为img显示" — the canvas IS the one and only visual source of truth
   * for that side, magnifier included, not just the static display). */
  canvasRef: React.RefObject<HTMLCanvasElement | null>;
  /** Which side (`"a"` or `"b"`) this canvas represents — fixed per sample (determined purely by
   * which side has fewer real pixels, per [`needsSyntheticPad`]), independent of whichever side
   * the user currently has toggled to. */
  paddedSide: ComparisonSide;
  alignment: CropAlignment;
  /** Whether `paddedSide` needs the synthetic pad at all — computed once by the caller via
   * [`needsSyntheticPad`] and shared with its own magnifier pointer-math wiring, so both agree on
   * the same decision (see that function's own docs for why this isn't re-derived here from a
   * live `<img>` ref). */
  needsPad: boolean;
  /** The OTHER side's own real pixel dimensions (`sample.a_width`/`a_height` or
   * `b_width`/`b_height`, whichever isn't `paddedSide`) — the canvas buffer's own resolution. */
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

      // Stripe the ENTIRE buffer first, then draw the real image on top — simpler than (and
      // avoids any seam/rounding gap between) computing 4 separate edge-band rects individually;
      // whatever area the image draw doesn't cover is exactly the synthetic-border area, no matter
      // how that area's shape works out from the band fractions.
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
