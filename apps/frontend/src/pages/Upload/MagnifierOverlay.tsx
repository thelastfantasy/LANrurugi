import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import type { CropAlignment } from "@/api/types";

import type { PadGeometry } from "./AlignmentBandOverlay";

const MAGNIFIER_SIZE_PX = 180;
const MAGNIFIER_ZOOM = 2.5;
// Offset away from the touched/hovered point so a holding finger doesn't cover the lens; the
// sampled pixel itself stays exactly where the finger is. Horizontal direction flips by viewport half.
const LENS_OFFSET_MARGIN_PX = 60;
const LENS_OFFSET_Y_PX = 140;

/** The actual on-screen box `objectFit: "contain"` renders an image's pixels into — a strict
 * subset of the `<img>`'s own layout box whenever aspect ratios don't match (letterboxing). */
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

/** A UV-normalized point — the canonical anchor form, independent of which side is displayed, so
 * it survives an A/B toggle without the sampled content jumping. */
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

/** Converts a client-space touch/mouse point into a UV-normalized anchor in A's own coordinate
 * space. `pad` (non-null) means the point uses the canvas's pad-offset transform instead of `containedImageRect`. */
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

/** A resolved lens rendering point — `lensLeft`/`lensTop` position the lens on-screen (offset from
 * the anchor); `sourceX`/`sourceY`/`sourceSize` are a square crop window in the source's real pixel space. */
export interface MagnifierPoint {
  lensLeft: number;
  lensTop: number;
  sourceX: number;
  sourceY: number;
  sourceSize: number;
}

/** Resolves an anchor (in A's own UV space) into lens rendering params. When `pad` is active,
 * `sampleUv` must be un-normalized by `pad.contentWidth`/`contentHeight`, not `sideNaturalWidth`/`Height`. */
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
    // `sampleUv` is already pad-normalized — un-normalize via `pad.content*`, not `sideNatural*`.
    sourceCenterX = (pad.offsetX + sampleUv.u * pad.contentWidth) * sourceWidth;
    sourceCenterY = (pad.offsetY + sampleUv.v * pad.contentHeight) * sourceHeight;
  } else {
    sourceCenterX = sampleUv.u * sideNaturalWidth;
    sourceCenterY = sampleUv.v * sideNaturalHeight;
  }
  const displayScale = sourceWidth / boxRect.width;
  const sourceSize = (MAGNIFIER_SIZE_PX / MAGNIFIER_ZOOM) * displayScale;
  // Direction flips by viewport half (lens is `position: fixed`), so it doesn't run off-screen.
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

/** Circular pixel-level magnifier — draws a cropped, zoomed region of `source` via `drawImage`.
 * Portaled to `document.body` so its stacking/clipping is independent of the image's ancestors. */
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

    // `drawImage` throws if `source` is a canvas with 0 width/height (a real race across an A/B
    // toggle) or an `<img>` that hasn't finished loading.
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

/** Tracks the current anchor plus the raw screen position, resolving both into `MagnifierPoint`
 * render params. Exposing the anchor separately lets `OverlayPage` re-resolve on an A/B toggle. */
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

  /** Re-resolves the lens reusing the last known anchor/screen position — for an A/B toggle with
   * no new pointer event. */
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
