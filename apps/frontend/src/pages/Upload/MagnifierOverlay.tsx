import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { CropAlignment } from "@/api/types";

import type { PadGeometry } from "./AlignmentBandOverlay";

const MAGNIFIER_SIZE_PX = 180;
const MAGNIFIER_ZOOM = 2.5;
// The lens is offset away from the actual touched/hovered point so a finger holding the image
// doesn't sit directly under (and hide) the lens itself — the *sampled* pixel is still exactly
// where the finger is, only the lens's own on-screen position is nudged. Horizontal direction
// flips based on which half of the viewport the touch is in (see `resolveLensPoint`) — a fixed
// "always offset right" pushed the lens off-screen near the right edge (reported live: "我指向图
// 片最右边你还往手指右上方偏移？"). Vertical stays a fixed upward offset.
const LENS_OFFSET_MARGIN_PX = 60;
const LENS_OFFSET_Y_PX = 140;

/** The actual on-screen box `objectFit: "contain"` renders an image's pixels into — a strict
 * subset of the `<img>` element's own layout box whenever the image's aspect ratio doesn't match
 * the box's (manga pages are usually taller/narrower than their container, leaving letterboxed
 * space on the sides). Needed because the magnifier must position/scale itself against where the
 * pixels actually are, not the element's full box. Exported for `AlignmentBandOverlay`'s own
 * identical need (positioning its border-band markers against the real rendered pixels, not the
 * `<img>` element's own possibly-letterboxed layout box). */
export function containedImageRect(img: HTMLImageElement) {
  const box = img.getBoundingClientRect();
  const naturalRatio = img.naturalWidth / img.naturalHeight;
  const boxRatio = box.width / box.height;
  let width: number;
  let height: number;
  if (naturalRatio > boxRatio) {
    width = box.width;
    height = box.width / naturalRatio;
  } else {
    height = box.height;
    width = box.height * naturalRatio;
  }
  const left = box.left + (box.width - width) / 2;
  const top = box.top + (box.height - height) / 2;
  return { left, top, width, height };
}

/** A UV-normalized point (per-own-dimension, matching `CropAlignment`'s own convention: x/width,
 * y/height) — the canonical form an "anchor" is stored in, independent of which side (A or B) is
 * currently displayed, so it survives an A/B toggle without the sampled content jumping to a
 * different part of the page. */
export interface UvPoint {
  u: number;
  v: number;
}

/** Applies `alignment` forward (A -> B). */
function applyAlignment(point: UvPoint, alignment: CropAlignment): UvPoint {
  return {
    u: point.u * alignment.scale + alignment.offset_x,
    v: point.v * alignment.scale + alignment.offset_y,
  };
}

/** Applies `alignment` in reverse (B -> A) — the inverse of `applyAlignment`. */
function invertAlignment(point: UvPoint, alignment: CropAlignment): UvPoint {
  return {
    u: (point.u - alignment.offset_x) / alignment.scale,
    v: (point.v - alignment.offset_y) / alignment.scale,
  };
}

/** Converts a raw client-space touch/mouse point into a UV-normalized anchor expressed in A's OWN
 * coordinate space, regardless of which side (`side`) is actually being hovered right now — this
 * is what lets the same anchor keep pointing at the same real content across an A/B toggle.
 *
 * `boxElement` is whichever element is ACTUALLY receiving the pointer event — the plain `<img>`
 * when no padding is active, or `AlignmentBandOverlay`'s own `<canvas>` directly when it is (that
 * canvas receives real pointer events for the padded side instead of a redundant covered-up
 * `<img>` sitting uselessly underneath it — confirmed live: "既然canvas本身就包含无白边图片，那么
 * 对应的img元素就不要放在DOM里了，多余"). `pad`, when non-null, means the pointer position must be
 * interpreted against the SAME box-relative + pad-offset transform the canvas itself used to draw
 * (the visually displayed content fills the box edge-to-edge, offset by `pad`'s own fractions, NOT
 * centered the way `object-fit: contain` on a plain, un-padded `<img>` would place it), rather
 * than `containedImageRect`'s letterbox math — which requires a real `<img>` (reads
 * `naturalWidth`/`naturalHeight` off it) and answers a different question anyway once a canvas is
 * the thing actually being interacted with. */
export function clientPointToAnchor(
  boxElement: HTMLElement,
  clientX: number,
  clientY: number,
  side: "a" | "b",
  alignment: CropAlignment,
  pad: PadGeometry | null,
): UvPoint {
  let u: number;
  let v: number;
  if (pad) {
    const box = boxElement.getBoundingClientRect();
    const fracX = Math.min(1, Math.max(0, (clientX - box.left) / box.width));
    const fracY = Math.min(1, Math.max(0, (clientY - box.top) / box.height));
    u = Math.min(1, Math.max(0, (fracX - pad.offsetX) / pad.contentWidth));
    v = Math.min(1, Math.max(0, (fracY - pad.offsetY) / pad.contentHeight));
  } else {
    const rect = containedImageRect(boxElement as HTMLImageElement);
    u = Math.min(1, Math.max(0, (clientX - rect.left) / rect.width));
    v = Math.min(1, Math.max(0, (clientY - rect.top) / rect.height));
  }
  return side === "a" ? { u, v } : invertAlignment({ u, v }, alignment);
}

/** A resolved lens rendering point — `lensLeft`/`lensTop` position the lens itself on-screen
 * (deliberately offset from the anchor's own screen position, see `LENS_OFFSET_X_PX`/
 * `LENS_OFFSET_Y_PX`, so a touch-holding finger doesn't cover the lens it's driving); `sourceX`/
 * `sourceY`/`sourceSize` describe a square crop window, in the SOURCE element's own real pixel
 * space (native `<img>` pixels, or the padded `<canvas>`'s own buffer pixels — see
 * `MagnifierOverlay`'s own docs for why it draws from whichever is the real visual source of
 * truth for this side, not always the `<img>`), that `MagnifierOverlay` draws scaled up to fill
 * the lens via `drawImage`. */
export interface MagnifierPoint {
  lensLeft: number;
  lensTop: number;
  sourceX: number;
  sourceY: number;
  sourceSize: number;
}

/** Resolves an anchor (in A's own UV space) into lens rendering params. `sourceWidth` is the
 * REAL SOURCE's own pixel width (the displayed side's own `naturalWidth` when `pad` is null, or
 * the padded canvas's own buffer width — i.e. `otherWidth` — when it's active); `boxRect` is the
 * on-screen CSS box the source is currently displayed within (whichever element is actually
 * receiving pointer events — plain `<img>` or `AlignmentBandOverlay`'s own `<canvas>` — see
 * `clientPointToAnchor`'s own docs for why either works identically here, same box shape thanks
 * to `OverlayPage`'s `aspectRatio` lock).
 *
 * When `pad` is active, `sampleUv` is NOT a plain UV over the padded side's own whole natural
 * frame — `clientPointToAnchor`'s own pad branch produces it as
 * `(fracY - pad.offsetY) / pad.contentHeight`, i.e. already divided by `pad.contentHeight`
 * (`CropAlignment.scale`), matching the SAME `applyAlignment`/`invertAlignment` relationship this
 * function's own non-pad branch and `clientPointToAnchor`'s own non-pad branch both use elsewhere.
 * An earlier version of this branch multiplied `sampleUv.v` directly by `sideNaturalHeight`
 * (skipping the `pad.contentHeight` factor entirely) on the reasoning that the padded side is
 * drawn unscaled onto the canvas, so its own real pixel height alone should be enough — that
 * reasoning missed that `sampleUv` itself already carries an implicit `/scale` from how the anchor
 * was computed on touchdown, so multiplying by `sideNaturalHeight` alone effectively multiplied by
 * `sideNaturalHeight/sourceHeight` instead of by `scale` — two DIFFERENT numbers whenever the
 * padded side's own aspect ratio isn't identical to the reference side's (the common case).
 * Confirmed live via injected `console.log` instrumentation on a real sample: `sideNaturalHeight/
 * sourceHeight` computed to 0.8929 while the real `scale` was 0.9183, producing a real ~20px (1.2%
 * of frame height) vertical mismatch between the padded-side and non-padded-side magnifier crops
 * for the exact same physical point — reported live as "自带白边图片的放大区域比canvas的放大区域偏
 * 上一点" (the plain-image side's magnified region sits noticeably higher than canvas's). Restoring
 * the `pad.offsetY`/`pad.contentHeight` un-normalization here (mirroring `clientPointToAnchor`'s
 * own formula exactly) is what makes the two branches agree. */
function resolveLensPoint(
  sourceWidth: number,
  sourceHeight: number,
  boxRect: { left: number; top: number; width: number; height: number },
  anchor: UvPoint,
  side: "a" | "b",
  alignment: CropAlignment,
  pad: PadGeometry | null,
  sideNaturalWidth: number,
  sideNaturalHeight: number,
  screenX: number,
  screenY: number,
  offsetLens: boolean,
): MagnifierPoint {
  const sampleUv = side === "a" ? anchor : applyAlignment(anchor, alignment);
  let sourceCenterX: number;
  let sourceCenterY: number;
  if (pad) {
    // Mirrors `clientPointToAnchor`'s own pad-branch relationship exactly (see this function's own
    // docs) — `sampleUv` here is already `/pad.contentWidth`-and-`/pad.contentHeight`-normalized,
    // so un-normalizing back to a buffer-relative fraction must multiply by `pad.contentWidth`/
    // `contentHeight` again, not by `sideNaturalWidth`/`sideNaturalHeight` (those describe the
    // padded side's own REAL resolution, a different number from the buffer-fraction `scale` this
    // branch needs — using them here was the actual bug, see this function's own docs).
    sourceCenterX = (pad.offsetX + sampleUv.u * pad.contentWidth) * sourceWidth;
    sourceCenterY = (pad.offsetY + sampleUv.v * pad.contentHeight) * sourceHeight;
  } else {
    // Here `sideNaturalWidth`/`sideNaturalHeight` ARE the right multiplier — this side isn't
    // padded, so `sampleUv` is a plain UV over its own whole natural frame with no `pad.content*`
    // normalization layered on top.
    sourceCenterX = sampleUv.u * sideNaturalWidth;
    sourceCenterY = sampleUv.v * sideNaturalHeight;
  }
  // A square crop window (both axes use the WIDTH-based display scale) — matches the lens's own
  // circular/square shape; using height's own scale too would only diverge from width's if the
  // box's aspect ratio didn't match the source's own (which it always does here, by construction:
  // `object-fit: contain` against a box whose `aspect-ratio` CSS is locked to the correct source).
  const displayScale = sourceWidth / boxRect.width;
  const sourceSize = (MAGNIFIER_SIZE_PX / MAGNIFIER_ZOOM) * displayScale;
  // Horizontal direction flips based on the touch/hover point's own position in the viewport (not
  // the image box, since the lens is `position: fixed` and clipped by the viewport) — a touch near
  // the right edge offsets LEFT instead of right, so the lens doesn't run off-screen or crowd the
  // edge. Vertical stays a fixed upward offset (the modal's own header/file-info area above the
  // image already keeps the lens clear of the viewport top).
  const offsetX = offsetLens
    ? (screenX > window.innerWidth / 2 ? -1 : 1) * LENS_OFFSET_MARGIN_PX
    : 0;
  const offsetY = offsetLens ? -LENS_OFFSET_Y_PX : 0;
  const lensLeft = screenX + offsetX - MAGNIFIER_SIZE_PX / 2;
  const lensTop = screenY + offsetY - MAGNIFIER_SIZE_PX / 2;
  return {
    lensLeft,
    lensTop,
    sourceX: sourceCenterX - sourceSize / 2,
    sourceY: sourceCenterY - sourceSize / 2,
    sourceSize,
  };
}

/** Circular pixel-level magnifier — draws a cropped, zoomed region of `source` (the real visual
 * source of truth for whichever side is currently displayed: the plain `<img>` when no padding is
 * active, or `AlignmentBandOverlay`'s own `<canvas>` when it is — see that component's own docs
 * for why the border-free side is never actually shown as an `<img>` once padding kicks in, so the
 * magnifier zooming into a separately-fetched raw image file would show DIFFERENT content than
 * what's on screen) onto a small lens canvas via `drawImage`, which works directly against either
 * element type without a network fetch. Portaled to `document.body` and positioned in viewport
 * space (`position: fixed`) rather than as a sibling of the image, so its own stacking/clipping is
 * independent of the image's ancestors. */
export function MagnifierOverlay({
  source,
  active,
  point,
}: {
  source: HTMLImageElement | HTMLCanvasElement | null;
  active: boolean;
  point: MagnifierPoint | null;
}) {
  const lensCanvasRef = useRef<HTMLCanvasElement | null>(null);

  useEffect(() => {
    const canvas = lensCanvasRef.current;
    if (!canvas || !active || !point || !source) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    // `drawImage` THROWS (InvalidStateError) if `source` is a canvas with a width or height of
    // 0 — a real, not just theoretical, race: `AlignmentBandOverlay`'s own canvas buffer is only
    // sized once ITS OWN effect draws (or resets to 0x0 when this side doesn't need padding), and
    // that can momentarily lag behind an A/B toggle (e.g. releasing Shift) re-rendering this
    // component with a `source` pointing at a canvas that hasn't been (re)drawn for the NEW side
    // yet — confirmed live: released Shift crashed the page here. Same guard for an `<img>` that
    // hasn't finished loading (`naturalWidth === 0`), for the same "source has no real pixels
    // yet" reason, even though that case is less likely to throw outright.
    const sourceWidth = source instanceof HTMLCanvasElement ? source.width : source.naturalWidth;
    const sourceHeight = source instanceof HTMLCanvasElement ? source.height : source.naturalHeight;
    if (sourceWidth === 0 || sourceHeight === 0) return;

    const dpr = window.devicePixelRatio || 1;
    canvas.width = MAGNIFIER_SIZE_PX * dpr;
    canvas.height = MAGNIFIER_SIZE_PX * dpr;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, MAGNIFIER_SIZE_PX, MAGNIFIER_SIZE_PX);
    ctx.drawImage(
      source,
      point.sourceX,
      point.sourceY,
      point.sourceSize,
      point.sourceSize,
      0,
      0,
      MAGNIFIER_SIZE_PX,
      MAGNIFIER_SIZE_PX,
    );
  }, [active, point, source]);

  if (!active || !point) return null;
  return createPortal(
    <div
      style={{
        position: "fixed",
        pointerEvents: "none",
        left: point.lensLeft,
        top: point.lensTop,
        width: MAGNIFIER_SIZE_PX,
        height: MAGNIFIER_SIZE_PX,
        borderRadius: "50%",
        border: "2px solid rgba(255,255,255,0.85)",
        boxShadow: "0 2px 10px rgba(0,0,0,0.5)",
        overflow: "hidden",
        zIndex: 9700,
      }}
    >
      <canvas
        ref={lensCanvasRef}
        style={{ width: MAGNIFIER_SIZE_PX, height: MAGNIFIER_SIZE_PX, display: "block" }}
      />
    </div>,
    document.body,
  );
}

/** Tracks the current anchor (in A's own UV space, see `UvPoint`'s own docs) plus the raw
 * screen-space position the lens itself should be drawn at, and resolves both — together with
 * whichever side/alignment/pad the caller passes in — into `MagnifierPoint` render params.
 * Exposing the anchor separately (not just a resolved point) is what lets `OverlayPage` re-resolve
 * the lens against a NEW side (on an A/B toggle) without needing a fresh mouse/touch event — the
 * anchor itself doesn't change when only the displayed side changes. */
export function usePointerPercent() {
  const [anchor, setAnchor] = useState<UvPoint | null>(null);
  const [screenPos, setScreenPos] = useState<{ x: number; y: number } | null>(null);
  const [point, setPoint] = useState<MagnifierPoint | null>(null);

  function percentFromClientPoint(
    boxElement: HTMLElement,
    clientX: number,
    clientY: number,
    side: "a" | "b",
    alignment: CropAlignment,
    offsetLens: boolean,
    pad: PadGeometry | null,
    sourceWidth: number,
    sourceHeight: number,
    sideNaturalWidth: number,
    sideNaturalHeight: number,
  ) {
    const nextAnchor = clientPointToAnchor(boxElement, clientX, clientY, side, alignment, pad);
    setAnchor(nextAnchor);
    setScreenPos({ x: clientX, y: clientY });
    const boxRect = boxElement.getBoundingClientRect();
    setPoint(
      resolveLensPoint(
        sourceWidth,
        sourceHeight,
        boxRect,
        nextAnchor,
        side,
        alignment,
        pad,
        sideNaturalWidth,
        sideNaturalHeight,
        clientX,
        clientY,
        offsetLens,
      ),
    );
  }

  /** Re-resolves the lens against a possibly-different `side`/`alignment`/`boxElement`, reusing
   * the last known anchor and screen position — for when `side` itself changes (A/B toggle)
   * without a new mouse/touch event to derive a fresh anchor from. */
  function reresolve(
    boxElement: HTMLElement,
    side: "a" | "b",
    alignment: CropAlignment,
    offsetLens: boolean,
    pad: PadGeometry | null,
    sourceWidth: number,
    sourceHeight: number,
    sideNaturalWidth: number,
    sideNaturalHeight: number,
  ) {
    if (!anchor || !screenPos) return;
    const boxRect = boxElement.getBoundingClientRect();
    setPoint(
      resolveLensPoint(
        sourceWidth,
        sourceHeight,
        boxRect,
        anchor,
        side,
        alignment,
        pad,
        sideNaturalWidth,
        sideNaturalHeight,
        screenPos.x,
        screenPos.y,
        offsetLens,
      ),
    );
  }

  function clear() {
    setAnchor(null);
    setScreenPos(null);
    setPoint(null);
  }

  return { point, percentFromClientPoint, reresolve, clear };
}
