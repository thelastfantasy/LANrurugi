import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import type { PageComparison } from "@/api/types";
import { formatBytes } from "@/components/Display";
import { useTouchMagnifyStore } from "@/store";
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme";

import { AlignmentBandOverlay, computePadGeometry, needsSyntheticPad } from "./AlignmentBandOverlay";
import { MagnifierOverlay, usePointerPercent } from "./MagnifierOverlay";

const TOUCH_HOLD_DELAY_MS = 150;
const SCROLL_INTENT_THRESHOLD_PX = 10;

/** One sample pair's A/B comparison; Shift (or touch hold) swaps to side B in place. */
export function OverlayPage({
  queueItemId,
  sample,
}: {
  queueItemId: string;
  sample: PageComparison;
}) {
  const { t } = useTranslation();
  const [showB, setShowB] = useState(false);
  const [badgeHovered, setBadgeHovered] = useState(false);
  const holdTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const imgRef = useRef<HTMLImageElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const preloadImgRef = useRef<HTMLImageElement | null>(null);
  const boxWrapperRef = useRef<HTMLDivElement | null>(null);
  const { point, percentFromClientPoint, reresolve, clear } = usePointerPercent();
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
  const bPreloadSrc = `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=b&index=${sample.b_page_index}`;

  const referenceIsA = sample.a_width * sample.a_height >= sample.b_width * sample.b_height;
  const referenceWidth = referenceIsA ? sample.a_width : sample.b_width;
  const referenceHeight = referenceIsA ? sample.a_height : sample.b_height;

  const aNeedsPad = needsSyntheticPad(sample.crop_alignment, sample.a_width, sample.a_height, sample.b_width, sample.b_height);
  const bNeedsPad = needsSyntheticPad(sample.crop_alignment, sample.b_width, sample.b_height, sample.a_width, sample.a_height);
  const paddedSide: "a" | "b" | null = aNeedsPad ? "a" : bNeedsPad ? "b" : null;
  const needsOwnBPreload = paddedSide !== "b";
  const paddedOtherWidth = paddedSide === "a" ? sample.b_width : sample.a_width;
  const paddedOtherHeight = paddedSide === "a" ? sample.b_height : sample.a_height;
  const paddedPageIndex = paddedSide === "b" ? sample.b_page_index : sample.a_page_index;
  const paddedSrc =
    paddedSide !== null
      ? `/api/download_queue/${encodeURIComponent(queueItemId)}/compare/page?side=${paddedSide}&index=${paddedPageIndex}`
      : null;

  const showingPaddedSide = paddedSide !== null && side === paddedSide;
  const padGeometry = paddedSide !== null ? computePadGeometry(paddedSide, sample.crop_alignment) : null;
  const activePadGeometry = showingPaddedSide ? padGeometry : null;
  const magnifierSourceWidth = showingPaddedSide ? otherWidth : width;
  const magnifierSourceHeight = showingPaddedSide ? otherHeight : height;
  const [magnifying, setMagnifying] = useState(false);
  const [magnifySource, setMagnifySource] = useState<"touch" | "mouse" | null>(null);
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

  function clearHoldTimer() {
    if (holdTimer.current !== null) {
      clearTimeout(holdTimer.current);
      holdTimer.current = null;
    }
  }
  const touchStartRef = useRef<{ x: number; y: number } | null>(null);
  const scrollCommittedRef = useRef(false);
  const lastTouchEndRef = useRef(0);
  // Refreshed every render — the touch `useEffect` below runs once (`[]` deps) and would
  // otherwise capture stale values.
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
      setTouchToggleOnRight(touch.clientX - rect.left < rect.width / 2);
      holdTimer.current = setTimeout(() => {
        holdTimer.current = null;
        setShowB(true);
        setMagnifying(true);
        setMagnifySource("touch");
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

      const latest = latestRef.current;
      if (!latest.magnifying) {
        clearHoldTimer();
        return;
      }
      const el = latest.activeElement();
      if (!el) return;
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
    // Ignore the synthetic post-touch `mousemove` mobile browsers dispatch.
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
    if (magnifySource === "touch") return;
    // A Shift toggle swaps the element under a stationary cursor, firing a spurious `mouseleave`.
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
        {needsOwnBPreload && (
          <img src={bPreloadSrc} alt="" style={{ display: "none" }} />
        )}
        {!showingPaddedSide && (
          <img
            ref={imgRef}
            src={src}
            alt=""
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
            <canvas
              ref={canvasRef}
              className="ai-compare-stripe-source"
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
            {t("upload.stripedAreaNotPartOf")}
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
          {t("upload.HeightSize", {
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
