import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { containedImageRect } from "@/pages/DuplicateCompare/MagnifierOverlay";

const LENS_SIZE_PX = 240;
const MIN_ZOOM = 1.5;
const MAX_ZOOM = 6;
const ZOOM_STEP = 0.5;

const DEFAULT_ZOOM = 2.5;
const ZOOM_KEY = "readerMagnifierZoom";

/** The last zoom picked in this browser. Unlike the on/off switch, it survives a reload. */
function storedZoom(): number {
  try {
    const value = Number(localStorage.getItem(ZOOM_KEY));
    return value >= MIN_ZOOM && value <= MAX_ZOOM ? value : DEFAULT_ZOOM;
  } catch {
    return DEFAULT_ZOOM;
  }
}

function storeZoom(zoom: number) {
  try {
    localStorage.setItem(ZOOM_KEY, String(zoom));
  } catch {
    // Storage blocked (private window): the zoom just resets on reload.
  }
}

/** `+`/`-` on the main row (`=`/`-` keys, with or without Shift) and on the numpad. */
function zoomDirection(e: KeyboardEvent): 1 | -1 | 0 {
  if (e.code === "NumpadAdd" || e.code === "Equal" || e.key === "+") return 1;
  if (e.code === "NumpadSubtract" || e.code === "Minus" || e.key === "-") return -1;
  return 0;
}

interface Lens {
  x: number;
  y: number;
  src: string;
  rect: { left: number; top: number; width: number; height: number };
}

/** The decoded page image under the pointer. Taken from the hit-test stack rather than the event
 * target, so translation boxes and markers drawn over a page (inside `#display`) do not hide it;
 * anything else in front of it — a dialog's backdrop, the overview, the toolbar — does. */
function pageImageAt(x: number, y: number): HTMLImageElement | null {
  for (const el of document.elementsFromPoint(x, y)) {
    if (!el.closest("#display")) return null;
    if (
      el instanceof HTMLImageElement &&
      el.classList.contains("reader-image") &&
      !el.classList.contains("loading-placeholder") &&
      el.naturalWidth > 0
    ) {
      return el;
    }
  }
  return null;
}

/** A lens centred on the mouse that shows the page under it enlarged (headless reader, toggled with
 * Space; `+`/`-` change the zoom). It samples the `<img>` already on screen, so it costs no extra request. */
export function ReaderMagnifier({ active }: { active: boolean }) {
  const [lens, setLens] = useState<Lens | null>(null);
  const [zoom, setZoom] = useState(storedZoom);
  const shownRef = useRef(false);
  // Tracked even while off, so switching on can place the lens before the mouse moves.
  const pointerRef = useRef<{ x: number; y: number } | null>(null);
  useEffect(() => {
    const track = (e: PointerEvent) => {
      if (e.pointerType === "mouse") pointerRef.current = { x: e.clientX, y: e.clientY };
    };
    window.addEventListener("pointermove", track, true);
    return () => window.removeEventListener("pointermove", track, true);
  }, []);
  useEffect(() => {
    shownRef.current = active && lens !== null;
  }, [active, lens]);

  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      // Ctrl/Cmd with +/- is the browser's own page zoom.
      if (e.ctrlKey || e.metaKey || e.altKey) return;
      if ((e.target as HTMLElement)?.tagName === "INPUT") return;
      // Only while the lens is showing: with a dialog over the page the keys belong to it.
      if (!shownRef.current) return;
      const direction = zoomDirection(e);
      if (direction === 0) return;
      e.preventDefault();
      setZoom((z) => {
        const next = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, z + direction * ZOOM_STEP));
        storeZoom(next);
        return next;
      });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [active]);

  useEffect(() => {
    if (!active) return;
    const update = () => {
      const last = pointerRef.current;
      if (!last) return;
      const img = pageImageAt(last.x, last.y);
      if (!img) {
        setLens(null);
        return;
      }
      const rect = containedImageRect(img);
      const inside =
        last.x >= rect.left &&
        last.x <= rect.left + rect.width &&
        last.y >= rect.top &&
        last.y <= rect.top + rect.height;
      setLens(inside ? { x: last.x, y: last.y, src: img.currentSrc || img.src, rect } : null);
    };
    const onMove = (e: PointerEvent) => {
      if (e.pointerType !== "mouse") return;
      update();
    };
    // A page turn or scroll moves the image under a pointer that has not moved.
    const onScroll = () => update();
    window.addEventListener("pointermove", onMove);
    // Show at once where the mouse already is, not on its next move.
    update();
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onScroll);
    const timer = window.setInterval(update, 250);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onScroll);
      window.clearInterval(timer);
      setLens(null);
    };
  }, [active]);

  if (!active || !lens) return null;
  const { x, y, src, rect } = lens;
  const half = LENS_SIZE_PX / 2;
  return createPortal(
    <div
      aria-hidden="true"
      // The rim colour comes from each theme file's `.reader-magnifier`.
      className="reader-magnifier"
      style={{
        position: "fixed",
        left: x - half,
        top: y - half,
        width: LENS_SIZE_PX,
        height: LENS_SIZE_PX,
        borderRadius: "50%",
        borderWidth: 2,
        borderStyle: "solid",
        boxShadow: "0 2px 12px rgba(0, 0, 0, 0.5)",
        pointerEvents: "none",
        zIndex: 2147483000,
        backgroundImage: `url("${src.replace(/"/g, '\\"')}")`,
        backgroundOrigin: "border-box",
        backgroundRepeat: "no-repeat",
        backgroundSize: `${rect.width * zoom}px ${rect.height * zoom}px`,
        backgroundPosition: `${half - (x - rect.left) * zoom}px ${half - (y - rect.top) * zoom}px`,
      }}
    />,
    document.body,
  );
}
