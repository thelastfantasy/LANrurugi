import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { PageComparison } from "@/api/types";
import { formatBytes } from "@/components/Display";
import { useTouchMagnifyStore } from "@/store";
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme";

import { AlignmentBandOverlay, computePadGeometry, needsSyntheticPad } from "./AlignmentBandOverlay";
import { MagnifierOverlay, usePointerPercent } from "./MagnifierOverlay";

// A finger down on the image could be the start of a hold-to-compare, or the start of a
// scroll-through-the-grid swipe — indistinguishable at touchstart. Deferring the B-swap behind
// this delay (canceled by any touchmove before it fires) lets a real scroll pass through
// untouched, matching jellyfin-suite's `FrameGrid.tsx` long-press-vs-drag disambiguation.
const TOUCH_HOLD_DELAY_MS = 150;
// How far (px) a touch may drift from its `touchstart` point before it's treated as a genuine
// scroll swipe rather than a still-finger hold-in-progress — used by the native, non-passive
// `touchmove` listener below to decide whether THIS move should still call `preventDefault()`.
// Must be checked on every move from the very first one (not gated behind `TOUCH_HOLD_DELAY_MS`
// the way the hold-timer cancellation is) since a browser commits to native scrolling from a touch
// sequence's first `touchmove`, not its 150ms-later state.
const SCROLL_INTENT_THRESHOLD_PX = 10;

/** One sample pair's overlay comparison — shows side A by default, holding Shift swaps to side B
 * in place so the user can compare without losing their eye's position. Caption shows this page's
 * own filename (`sample.a_filename`/`b_filename`), not the whole archive's.
 *
 * The magnifier's sampled point tracks real page CONTENT across an A/B toggle, not just the same
 * raw screen position — `sample.crop_alignment` corrects for the two scans having a different
 * crop margin and/or resolution (a common real scenario), so holding Shift over some detail in A
 * still shows that same detail in B even if B's scan has e.g. a wider white border. */
export function OverlayPage({
  queueItemId,
  sample,
}: {
  queueItemId: string;
  sample: PageComparison;
}) {
  const { t } = useTranslation();
  const [showB, setShowB] = useState(false);
  // Badge fades on hover so it doesn't obscure image content underneath.
  const [badgeHovered, setBadgeHovered] = useState(false);
  const holdTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const preloadImgRef = useRef<HTMLImageElement | null>(null);
  const boxWrapperRef = useRef<HTMLDivElement | null>(null);
  const { point, percentFromClientPoint, reresolve, clear } = usePointerPercent();
  // Matches ComparisonResultModal's own React `key` for this sample.
  const sampleKey = `${sample.a_page_index}-${sample.b_page_index}`;
  const setActive = useTouchMagnifyStore((s) => s.setActive);

  const side = showB ? "b" : "a";
  const pageIndex = showB ? sample.b_page_index : sample.a_page_index;
  const sharpness = showB ? sample.b_sharpness : sample.a_sharpness;
  const width = showB ? sample.b_width : sample.a_width;
  const height = showB ? sample.b_height : sample.a_height;
  const fileSize = showB ? sample.b_file_size : sample.a_file_size;
  const src = `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${side}&index=${pageIndex}`;
  const otherWidth = showB ? sample.a_width : sample.b_width;
  const otherHeight = showB ? sample.a_height : sample.b_height;
  // B's own page — a FIXED src (never swaps with the `showB` toggle, unlike `src` above), always
  // preloaded via a hidden `<img>` below regardless of whether padding is involved at all, so the
  // very first Shift press doesn't cold-fetch a fresh, un-decoded ~250-450KB page image with zero
  // head start. Real, measured jank (not guessed): a Chrome DevTools performance trace of a real
  // first Shift press showed the `ImageDelivery` insight firing exactly inside that interaction's
  // own window — all 6 of a sample grid's B-side images starting their network fetch only at the
  // moment of the keypress itself, ~1.5MB combined, straight into a single INP-affecting
  // interaction. A is never displayed until it exists (this component itself only mounts once its
  // sample data is already in hand), so only B needs its own dedicated preload here — the existing
  // `paddedSrc` preload below only ever covered the synthetic-border case, leaving every ordinary
  // (non-padded) sample's B side with no preload at all. Skipped (via `needsOwnBPreload` below,
  // computed after `paddedSide` is known) when B IS the padded side — `paddedSrc` already preloads
  // that exact same URL, and mounting a second `<img>` for it would just be a redundant fetch.
  const bPreloadSrc = `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=b&index=${sample.b_page_index}`;

  // The side with MORE real pixels is the one `AlignmentBandOverlay` treats as "the reference
  // frame" (see that component's own docs) — the smaller side gets a synthetic pad drawn to match
  // it. Locking the DISPLAYED BOX's own aspect ratio to that reference frame, regardless of which
  // side is CURRENTLY shown, is what actually makes the two sides occupy the same box shape when
  // toggled: without this, the plain `<img>` element's own natural `object-fit: contain` sizing
  // recomputes fresh from whichever side's `src` is currently loaded, so the box itself changed
  // shape/size across an A/B toggle even though `AlignmentBandOverlay`'s own canvas buffer was
  // already correctly sized — confirmed live (reported directly: "canvas和带白边的图片分辨率必须
  // 一样") that a per-side box, even with an internally-correct canvas, still doesn't visually
  // overlap-align against the other side's own differently-shaped box.
  const referenceIsA = sample.a_width * sample.a_height >= sample.b_width * sample.b_height;
  const referenceWidth = referenceIsA ? sample.a_width : sample.b_width;
  const referenceHeight = referenceIsA ? sample.a_height : sample.b_height;

  // Which side (if either) needs the synthetic pad — a FIXED property of the sample, independent
  // of the current A/B toggle (`side` above) — "whichever side has fewer real pixels becomes the
  // canvas" is data-driven, decided once, not re-derived per toggle. Checking A-against-B and
  // B-against-A separately (rather than reusing `side`/`otherWidth` above) is what makes this
  // toggle-independent: at most one of the two can ever be true for a real (non-identity)
  // alignment, per `needsSyntheticPad`'s own docs.
  const aNeedsPad = needsSyntheticPad(sample.crop_alignment, sample.a_width, sample.a_height, sample.b_width, sample.b_height);
  const bNeedsPad = needsSyntheticPad(sample.crop_alignment, sample.b_width, sample.b_height, sample.a_width, sample.a_height);
  const paddedSide: "a" | "b" | null = aNeedsPad ? "a" : bNeedsPad ? "b" : null;
  // See `bPreloadSrc`'s own docs — only mount that dedicated preload `<img>` when B ISN'T already
  // covered by `paddedSrc` below (same URL either way, so a second `<img>` would just duplicate
  // the fetch).
  const needsOwnBPreload = paddedSide !== "b";
  const paddedOtherWidth = paddedSide === "a" ? sample.b_width : sample.a_width;
  const paddedOtherHeight = paddedSide === "a" ? sample.b_height : sample.a_height;
  const paddedPageIndex = paddedSide === "b" ? sample.b_page_index : sample.a_page_index;
  // A FIXED src (never changes with the A/B toggle) — see `preloadImgRef`'s own docs for why this
  // dedicated `<img>` exists at all: it lets the padded side's real image bytes start
  // downloading/decoding the moment this component mounts (modal open), rather than only once the
  // user's first Shift press makes `paddedSide` the CURRENTLY DISPLAYED one.
  const paddedSrc =
    paddedSide !== null
      ? `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${paddedSide}&index=${paddedPageIndex}`
      : null;

  // Is the CURRENTLY DISPLAYED side the padded one? — this (not `paddedSide !== null` alone)
  // gates the magnifier's own pad-aware math and the canvas's own visibility below, since
  // `AlignmentBandOverlay`'s canvas is kept current in the background regardless of the toggle,
  // but should only actually be SHOWN (and sampled by the magnifier) while the user has toggled
  // to view that specific side.
  const showingPaddedSide = paddedSide !== null && side === paddedSide;
  const padGeometry = paddedSide !== null ? computePadGeometry(paddedSide, sample.crop_alignment) : null;
  const activePadGeometry = showingPaddedSide ? padGeometry : null;
  // The magnifier's own real visual source of truth: the padded `<canvas>` (its own buffer
  // resolution == `otherWidth`/`otherHeight`) when padding is active — the border-free side is
  // NEVER actually shown as a plain `<img>` once padding kicks in (confirmed live: "这张没白边的
  // 图片根本不作为img显示"), so zooming into a separately-fetched raw image file via CSS
  // background would show DIFFERENT content than what's on screen. Otherwise the plain `<img>`
  // itself, at its own native resolution.
  const magnifierSourceWidth = showingPaddedSide ? otherWidth : width;
  const magnifierSourceHeight = showingPaddedSide ? otherHeight : height;
  // Desktop: hover shows the magnifier. Touch: the held position drives it instead, and — since
  // touch has no separate "hold" vs "release-to-flip" gesture the way desktop's Shift key does —
  // magnifying is what actually keeps A/B visible/toggleable while the finger stays down (see
  // `magnifying`'s own role below vs. `showB`).
  const [magnifying, setMagnifying] = useState(false);
  // Distinguishes a touch-triggered magnify (needs the manual A/B toggle button, since there's no
  // momentary-release gesture) from a desktop mouse-hover one (Shift already covers A/B swapping,
  // so the touch-only toggle button must not appear there).
  const [magnifySource, setMagnifySource] = useState<"touch" | "mouse" | null>(null);
  // Which side of the image the long-press started on — used to park the touch A/B toggle button
  // on the *opposite* side, so the thumb doing the holding doesn't cover it. Touch has no way to
  // know which hand is actually pressing, so this is a heuristic, not real hand detection.
  const [touchToggleOnRight, setTouchToggleOnRight] = useState(true);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Shift") setShowB(true);
    }
    function onKeyUp(e: KeyboardEvent) {
      if (e.key === "Shift") setShowB(false);
    }
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("keyup", onKeyUp);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("keyup", onKeyUp);
    };
  }, []);

  useEffect(() => {
    return () => {
      if (holdTimer.current !== null) clearTimeout(holdTimer.current);
    };
  }, []);

  // React attaches JSX `onTouchMove` as a PASSIVE native listener (since React 17), so
  // `e.preventDefault()` inside a JSX touch handler is silently a no-op — the browser is free to
  // treat finger movement as a native page scroll regardless of what that handler does. Reported
  // live: "手指按住第一张图后，页面本身会跟着手指一起滚动", and the resulting desync between the
  // scrolling element's `getBoundingClientRect()` and the touch's own `clientY` degrades the lens's
  // effective zoom toward 1:1 ("圆圈还在，但圆圈里面的内容变成了未放大的原图").
  //
  // Touch handlers are bound HERE, natively on `boxWrapperRef` (not via JSX `onTouchStart`/etc on
  // the individual `<img>`/`<canvas>` elements) so this one binding covers both regardless of which
  // is currently visible — `boxWrapperRef` itself never unmounts or changes identity. An earlier
  // version bound the SAME handlers both ways at once (this native listener here, plus the old JSX
  // handlers still left on `<img>`/`<canvas>`) — two independent copies of the hold/magnify state
  // machine racing each other on the same real touch sequence, which is what actually produced
  // "shows the unmagnified original after moving" (confirmed live, sample-index-specific: only
  // reproduced on the one sample whose `<canvas>` was the initially-active element, since that's
  // the shape that made the two copies' own internal state diverge soonest). Fixed by deleting the
  // JSX-bound copies entirely — this is now the only binding.
  function clearHoldTimer() {
    if (holdTimer.current !== null) {
      clearTimeout(holdTimer.current);
      holdTimer.current = null;
    }
  }
  const touchStartRef = useRef<{ x: number; y: number } | null>(null);
  const scrollCommittedRef = useRef(false);
  // Set on every real `touchend` — `handleMouseMove` checks this to ignore the synthetic
  // `mousemove` mobile browsers dispatch right after ANY touch (tap or hold, see that handler's
  // own docs). Without it, a plain tap (never long enough to become a real hold, so `magnifySource`
  // is still `null`) fell through to `handleMouseMove`'s own `magnifySource === "touch"` guard
  // (which only protects an ALREADY-committed touch hold) and opened the magnifier anyway.
  const lastTouchEndRef = useRef(0);
  // A snapshot of every render-derived value/function the touch handlers below need, refreshed on
  // EVERY render (not memoized) — the touch-handling `useEffect` itself only runs once (`[]` deps,
  // see its own docs for why: the native listener must never be torn down/re-attached mid-gesture),
  // so its closures would otherwise see whichever values existed at the first render forever. Read
  // via `latestRef.current` inside the native handlers instead of capturing these directly.
  const latestRef = useRef<{
    side: "a" | "b";
    sample: PageComparison;
    magnifying: boolean;
    activePadGeometry: ReturnType<typeof computePadGeometry> | null;
    magnifierSourceWidth: number;
    magnifierSourceHeight: number;
    activeElement: () => HTMLElement | null;
    sideNaturalSize: () => { width: number; height: number };
    percentFromClientPoint: typeof percentFromClientPoint;
    clear: typeof clear;
  }>({
    side,
    sample,
    magnifying,
    activePadGeometry,
    magnifierSourceWidth,
    magnifierSourceHeight,
    activeElement: () => null,
    sideNaturalSize: () => ({ width: 0, height: 0 }),
    percentFromClientPoint,
    clear,
  });
  latestRef.current = {
    side,
    sample,
    magnifying,
    activePadGeometry,
    magnifierSourceWidth,
    magnifierSourceHeight,
    activeElement,
    sideNaturalSize,
    percentFromClientPoint,
    clear,
  };
  useEffect(() => {
    const wrapper = boxWrapperRef.current;
    if (!wrapper) return;

    function onTouchStartNative(e: TouchEvent) {
      clearHoldTimer();
      const touch = e.touches[0];
      touchStartRef.current = { x: touch.clientX, y: touch.clientY };
      scrollCommittedRef.current = false;
      const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
      // Long-press started left half → park the toggle on the right (out from under a right-thumb
      // reaching in from the bottom-right), and vice versa.
      setTouchToggleOnRight(touch.clientX - rect.left < rect.width / 2);
      holdTimer.current = setTimeout(() => {
        holdTimer.current = null;
        setShowB(true);
        setMagnifying(true);
        setMagnifySource("touch");
        // Starting a fresh hold always begins on side A's own element (pre-toggle) — `side: "a"`.
        // `activePadGeometry`/`magnifierSourceWidth`/`magnifierSourceHeight` (from `latestRef`,
        // see its own docs) already correspond to "a" here too: `showB` is always false at the
        // moment a fresh hold starts (a prior hold always resets it before another can begin).
        const latest = latestRef.current;
        const el = latest.activeElement();
        if (!el) return;
        const { width: nw, height: nh } = latest.sideNaturalSize();
        latest.percentFromClientPoint(
          el,
          touch.clientX,
          touch.clientY,
          "a",
          latest.sample.crop_alignment,
          true,
          latest.activePadGeometry,
          latest.magnifierSourceWidth,
          latest.magnifierSourceHeight,
          nw,
          nh,
        );
      }, TOUCH_HOLD_DELAY_MS);
    }

    function onTouchMoveNative(e: TouchEvent) {
      const start = touchStartRef.current;
      const touch = e.touches[0];
      if (!scrollCommittedRef.current && start) {
        const dx = touch.clientX - start.x;
        const dy = touch.clientY - start.y;
        if (Math.hypot(dx, dy) > SCROLL_INTENT_THRESHOLD_PX) {
          scrollCommittedRef.current = true;
        }
      }
      if (!scrollCommittedRef.current) e.preventDefault();

      // Cancels the pending hold (and prevents ever showing B) the moment a real scroll swipe is
      // detected before the hold commits — lets the browser take over native scrolling for the
      // rest of the gesture instead of the image eating the touch. Once magnifying has already
      // committed, movement instead repositions the lens (finger sliding around the same held
      // page).
      const latest = latestRef.current;
      if (!latest.magnifying) {
        clearHoldTimer();
        return;
      }
      const el = latest.activeElement();
      if (!el) return;
      // Keeps the fixed toggle button on the side opposite the CURRENT finger position, not just
      // where the hold started — a long-press finger sliding across the image (e.g. left half to
      // right half) previously left the button parked on its original side, still covered by the
      // hand that moved there (reported live: "放大镜从左边拖到右边后，该按钮还是在右边").
      const rect = el.getBoundingClientRect();
      setTouchToggleOnRight(touch.clientX - rect.left < rect.width / 2);
      const { width: nw, height: nh } = latest.sideNaturalSize();
      latest.percentFromClientPoint(
        el,
        touch.clientX,
        touch.clientY,
        latest.side,
        latest.sample.crop_alignment,
        true,
        latest.activePadGeometry,
        latest.magnifierSourceWidth,
        latest.magnifierSourceHeight,
        nw,
        nh,
      );
    }

    function onTouchEndNative() {
      clearHoldTimer();
      setShowB(false);
      setMagnifying(false);
      setMagnifySource(null);
      latestRef.current.clear();
      lastTouchEndRef.current = Date.now();
    }

    wrapper.addEventListener("touchstart", onTouchStartNative, { passive: true });
    wrapper.addEventListener("touchmove", onTouchMoveNative, { passive: false });
    wrapper.addEventListener("touchend", onTouchEndNative, { passive: true });
    wrapper.addEventListener("touchcancel", onTouchEndNative, { passive: true });
    return () => {
      wrapper.removeEventListener("touchstart", onTouchStartNative);
      wrapper.removeEventListener("touchmove", onTouchMoveNative);
      wrapper.removeEventListener("touchend", onTouchEndNative);
      wrapper.removeEventListener("touchcancel", onTouchEndNative);
    };
  }, []);

  // Registers this sample with the shared touch-magnify store while active, so the modal's one
  // fixed toggle button can act on it.
  const touchToggleVisible = magnifySource === "touch";
  useEffect(() => {
    if (!touchToggleVisible) return;
    setActive({
      sampleKey,
      showB,
      toggleSide: () => setShowB((v) => !v),
      toggleOnRight: touchToggleOnRight,
    });
    return () => setActive(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [touchToggleVisible, showB, touchToggleOnRight]);

  // Whichever element is CURRENTLY the real interactive/visual surface for this sample — the
  // padded `<canvas>` while showing the padded side (it receives real pointer events directly;
  // the `<img>` underneath it isn't even rendered — confirmed live: "既然canvas本身就包含无白边图
  // 片，那么对应的img元素就不要放在DOM里了，多余"), or the plain toggle-driven `<img>` otherwise.
  // `sideNaturalWidth`/`sideNaturalHeight` — the CURRENTLY DISPLAYED side's own real resolution,
  // needed by the magnifier's pad-branch math — come from the dedicated preload `<img>` (always
  // already loaded by the time padding is actually being shown) rather than a toggle-driven one
  // that, in the padded case, no longer exists in the DOM at all.
  function activeElement(): HTMLElement | null {
    return showingPaddedSide ? canvasRef.current : imgRef.current;
  }
  function sideNaturalSize(): { width: number; height: number } {
    if (showingPaddedSide) {
      const img = preloadImgRef.current;
      return { width: img?.naturalWidth ?? 0, height: img?.naturalHeight ?? 0 };
    }
    return { width: imgRef.current?.naturalWidth ?? 0, height: imgRef.current?.naturalHeight ?? 0 };
  }

  // Re-resolves the lens's sampled content whenever the displayed side flips (Shift down/up, or
  // the touch toggle button) WITHOUT a fresh mouse/touch event to derive a new anchor from — the
  // physical cursor/finger hasn't moved, so the anchor itself (in A's own space) is unchanged, but
  // the CONTENT it should now sample shifts to B's own coordinates via `crop_alignment` once the
  // displayed side swaps. Without this, toggling sides left the magnifier showing stale
  // positioning math computed against whichever side was active at the last real pointer event,
  // silently misaligned by exactly the crop/scale difference between A and B.
  //
  // Also re-runs on `sample.crop_alignment` changing WITHOUT a side toggle — a streamed `"precise"`
  // event can replace this sample's `crop_alignment` while the user is actively holding the
  // magnifier open on it (the coarse→precise refinement now streams sample-by-sample instead of
  // all at once); without this, the magnifier kept sampling against the stale coarse alignment
  // until the next toggle/pointer event happened to re-trigger it.
  useEffect(() => {
    const el = activeElement();
    if (!magnifying || !el) return;
    const { width: nw, height: nh } = sideNaturalSize();
    reresolve(
      el,
      side,
      sample.crop_alignment,
      magnifySource === "touch",
      activePadGeometry,
      magnifierSourceWidth,
      magnifierSourceHeight,
      nw,
      nh,
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [showB, sample.crop_alignment]);

  // Safety net for the case above: an `<img>`/`<canvas>` swapping content doesn't necessarily
  // have its final pixels ready the instant `showB` flips (network-fetched page images, not
  // always already cached) — if `reresolve` ran before that finished, it silently used stale
  // dimensions. Once the relevant element's own `load` event fires, re-resolve again.
  function handleImageLoad() {
    const el = activeElement();
    if (!magnifying || !el) return;
    const { width: nw, height: nh } = sideNaturalSize();
    reresolve(
      el,
      side,
      sample.crop_alignment,
      magnifySource === "touch",
      activePadGeometry,
      magnifierSourceWidth,
      magnifierSourceHeight,
      nw,
      nh,
    );
  }

  function handleMouseMove(e: React.MouseEvent<HTMLElement>) {
    // Mobile browsers dispatch a synthetic `mousemove` right after ANY real touch (tap or hold —
    // legacy compat behavior for mouse-only pages), not just during an already-committed hold.
    // `magnifySource === "touch"` alone only guards the latter case (mid-hold); a plain tap never
    // reaches `TOUCH_HOLD_DELAY_MS` so `magnifySource` is still `null` when its own synthetic
    // `mousemove` arrives, which fell through and opened the magnifier anyway (reported live: a
    // plain tap on the image — no hold — showed the magnifier immediately). `lastTouchEndRef`
    // catches this second case: ignore any `mousemove` landing within 500ms of a real `touchend`.
    if (magnifySource === "touch") return;
    if (Date.now() - lastTouchEndRef.current < 500) return;
    const { width: nw, height: nh } = sideNaturalSize();
    percentFromClientPoint(
      e.currentTarget,
      e.clientX,
      e.clientY,
      side,
      sample.crop_alignment,
      false,
      activePadGeometry,
      magnifierSourceWidth,
      magnifierSourceHeight,
      nw,
      nh,
    );
    setMagnifying(true);
    setMagnifySource("mouse");
  }

  function handleMouseLeave(e: React.MouseEvent<HTMLElement>) {
    // Mobile browsers dispatch a synthetic `mouseleave` alongside real touch events too (same
    // legacy compat behavior `handleMouseMove`'s own touch guard already documents) — a touch-
    // driven A/B toggle swapping the visible `<img>`/`<canvas>` triggers exactly this, and since
    // this handler unconditionally cleared `magnifySource`/`magnifying`, tapping the touch-only
    // toggle button made the lens (and the button itself, gated on `magnifySource === "touch"`)
    // disappear right after a successful switch (reported live: "能切换了，但是切换后按钮消失了").
    // A touch-driven session owns its own lifecycle via `onTouchEndNative` (`boxWrapperRef`'s own
    // native listener) — this handler has no business clearing touch state at all.
    if (magnifySource === "touch") return;
    // Pressing Shift swaps the CURRENT interactive element (the plain `<img>` unmounts, the padded
    // `<canvas>` takes over, or vice versa on release) — the browser fires a real `mouseleave` on
    // the outgoing element for this even though the cursor itself never actually left the sample's
    // own box, since the element under a STATIONARY cursor just changed identity. Reported live:
    // "按下shift的时候放大镜会消失，需要光标移动后才会重新出现" — clearing the magnifier here
    // (`setMagnifying(false)`/`clear()`) hid the lens until a real subsequent `mousemove` on the
    // new element repopulated it; the `[showB]` effect's own `reresolve` call (meant to handle
    // exactly this transition) never got a chance to run against a still-`magnifying` state, since
    // this handler had already cleared it first. `e.relatedTarget` is where the cursor is now
    // headed — if that's still inside this sample's own wrapper box (the sibling `<img>`/`<canvas>`
    // included), the cursor hasn't really left, so skip clearing and let the swap complete under
    // the existing anchor/point instead.
    const related = e.relatedTarget;
    if (related instanceof Node && boxWrapperRef.current?.contains(related)) {
      return;
    }
    setMagnifying(false);
    setMagnifySource(null);
    clear();
  }

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        gap: 4,
        minWidth: 0,
      }}
    >
      <div
        ref={boxWrapperRef}
        style={{
          position: "relative",
          display: "flex",
          justifyContent: "center",
        }}
      >
        {/* Dedicated, toggle-INDEPENDENT preload for B's own page — fixed `src`, never swaps with
            the A/B toggle, so B's real bytes start downloading/decoding as soon as this sample
            mounts instead of only once the user's first Shift press asks for them (see
            `bPreloadSrc`'s own docs for the real, measured jank this fixes — a Chrome DevTools
            trace of a real first-Shift interaction showed all 6 grid samples' B images starting
            their fetch only at that exact keypress). Skipped when B is already the padded side —
            `paddedSrc` below already preloads that same URL. Never visible/interactive itself. */}
        {needsOwnBPreload && (
          <img src={bPreloadSrc} alt="" style={{ display: "none" }} />
        )}
        {/* The normal, toggle-driven side — only rendered while it's NOT the padded one, since
            the padded side is fully represented by the canvas below instead (rendering both would
            mean the img's own bytes decode for nothing, permanently hidden under the canvas —
            confirmed live: "既然canvas本身就包含无白边图片，那么对应的img元素就不要放在DOM里了，
            多余"). */}
        {!showingPaddedSide && (
          <img
            ref={imgRef}
            src={src}
            alt=""
            // Real touch handling (hold-then-magnify, long-press-vs-scroll disambiguation) is
            // bound natively on `boxWrapperRef` instead of here — see that `useEffect`'s own docs
            // for why: this `<img>` is conditionally rendered (`{!showingPaddedSide && ...}`), and
            // a committed long-press's own `setShowB(true)` can toggle `showingPaddedSide` mid-
            // gesture, which used to unmount whichever element the touch had started on. `touch-
            // action: manipulation` (not `none`) keeps native scroll/pan working outside a held
            // gesture. Long-press save/copy-image menu suppression needs both: `-webkit-touch-
            // callout: none` for iOS Safari (Android Chrome ignores that WebKit-only property
            // entirely), and `onContextMenu`/`preventDefault()` for Android Chrome, which fires a
            // real `contextmenu` event on long-press instead.
            onContextMenu={(e) => e.preventDefault()}
            onMouseMove={handleMouseMove}
            onMouseLeave={handleMouseLeave}
            onLoad={handleImageLoad}
            draggable={false}
            style={{
              maxHeight: "70vh",
              maxWidth: "100%",
              aspectRatio: `${referenceWidth} / ${referenceHeight}`,
              display: "block",
              borderRadius: 4,
              objectFit: "contain",
              touchAction: "manipulation",
              WebkitTouchCallout: "none",
              WebkitUserSelect: "none",
              userSelect: "none",
              cursor: "zoom-in",
            }}
          />
        )}
        {paddedSide !== null && (
          <>
            {/* Dedicated, toggle-INDEPENDENT preload — fixed `src`, never swaps with the A/B
                toggle. Lets the padded side's real bytes start downloading/decoding as soon as
                the modal opens instead of only once the user's first Shift press makes it the
                CURRENTLY DISPLAYED side (see `AlignmentBandOverlay`'s own docs for the real,
                measured decode-burst jank this fixes). Never visible/interactive itself — purely
                a decode source for `AlignmentBandOverlay`'s canvas draw and a `naturalWidth`/
                `naturalHeight` source for the magnifier's pad-branch math. */}
            <img
              ref={preloadImgRef}
              src={paddedSrc ?? undefined}
              alt=""
              onLoad={handleImageLoad}
              style={{ display: "none" }}
            />
            <AlignmentBandOverlay
              preloadImgRef={preloadImgRef}
              canvasRef={canvasRef}
              paddedSide={paddedSide}
              alignment={sample.crop_alignment}
              needsPad={true}
              otherWidth={paddedOtherWidth}
              otherHeight={paddedOtherHeight}
            />
            {/* The padded side's own real interactive/visual surface — real content, no separate
                `<img>` underneath (see above). Only actually visible/hit-testable while the user
                has toggled to `paddedSide`; otherwise `display: none` (still mounted, so
                `AlignmentBandOverlay`'s background draw above always has a canvas ref to draw
                into regardless of the current toggle). */}
            <canvas
              ref={canvasRef}
              // Carries no visible styling of its own — `AlignmentBandOverlay`'s draw effect reads
              // this class's `background-color`/`border-color` (declared per-theme, see that
              // class's own docs in each `apps/frontend/public/legacy/themes/*.css`) via
              // `getComputedStyle` to pick the stripe pattern's colors, since a `<canvas>` can't be
              // colored by CSS the way a normal element can — this is just a color SOURCE for the
              // JS draw call, not an actual applied style.
              className="ai-compare-stripe-source"
              // Touch handling bound on `boxWrapperRef` instead — see the `<img>` element's own
              // docs above for why (this canvas is the OTHER side of the same conditional-mount
              // problem: it's always mounted, but the `<img>` above isn't, so binding here alone
              // would still break a hold that started on the `<img>`).
              onContextMenu={(e) => e.preventDefault()}
              onMouseMove={handleMouseMove}
              onMouseLeave={handleMouseLeave}
              style={{
                display: showingPaddedSide ? "block" : "none",
                maxHeight: "70vh",
                maxWidth: "100%",
                aspectRatio: `${referenceWidth} / ${referenceHeight}`,
                borderRadius: 4,
                objectFit: "contain",
                touchAction: "manipulation",
                cursor: "zoom-in",
              }}
            />
          </>
        )}
        <MagnifierOverlay
          source={showingPaddedSide ? canvasRef.current : imgRef.current}
          active={magnifying}
          point={point}
        />
        {/* The striped area `AlignmentBandOverlay` draws isn't part of either scan — it's a
            placeholder filling the gap a synthetic border creates so the two sides' real content
            can visually align. Without this label, reported live: a striped region reads as either
            actual manga content or a rendering bug, not "there's nothing here on purpose". Only
            shown while the padded side is the one actually on screen — irrelevant otherwise. */}
        {showingPaddedSide && (
          <span
            style={{
              position: "absolute",
              top: 8,
              left: 8,
              maxWidth: "calc(100% - 16px)",
              padding: "3px 8px",
              borderRadius: 4,
              fontSize: FONT_SIZE_XS,
              background: "rgba(0,0,0,0.65)",
              color: "#fff",
              boxSizing: "border-box",
            }}
          >
            {t("Striped area: not part of the scan — filled in to align the two versions")}
          </span>
        )}
        <span
          onMouseEnter={() => setBadgeHovered(true)}
          onMouseLeave={() => setBadgeHovered(false)}
          style={{
            position: "absolute",
            top: 8,
            right: 8,
            width: 28,
            height: 28,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            borderRadius: "50%",
            fontSize: FONT_SIZE_SM,
            fontWeight: 700,
            background: showB
              ? "rgba(46,125,79,0.85)"
              : "rgba(58,124,199,0.85)",
            color: "#fff",
            opacity: badgeHovered ? 0 : 1,
            transition: "opacity 0.15s ease",
          }}
        >
          {showB ? "B" : "A"}
        </span>
      </div>
      <div
        className="ai-compare-caption-bg"
        style={{
          alignSelf: "stretch",
          boxSizing: "border-box",
          borderRadius: 4,
          padding: "6px 8px",
        }}
      >
        <div
          style={{
            fontSize: FONT_SIZE_SM,
            textAlign: "center",
            wordBreak: "break-all",
            opacity: 0.9,
          }}
        >
          {showB ? sample.b_filename : sample.a_filename}
        </div>
        <div
          style={{
            fontSize: FONT_SIZE_XS,
            textAlign: "center",
            opacity: 0.75,
            marginTop: 2,
          }}
        >
          {t("{{width}}×{{height}} · {{size}} · Sharpness: {{score}}", {
            width,
            height,
            size: formatBytes(fileSize),
            score: sharpness.toFixed(0),
          })}
        </div>
      </div>
    </div>
  );
}
