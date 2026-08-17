import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useKeepSideB, useOverwriteQueueItem } from "@/api/hooks";
import type { ExportPatchInsertion } from "@/api/types";
import { formatBytes, IconButton } from "@/components/Display";
import { useIsNarrowViewport } from "@/hooks";
import { createTouchMagnifyStore, TouchMagnifyStoreContext, useTouchMagnifyStore } from "@/store";
import { FONT_SIZE_MD, FONT_SIZE_SM } from "@/theme";
import { toast } from "@/toast";

import { EntryTreePopover } from "./EntryTreePopover";
import { KeepSideButton } from "./KeepSideButton";
import { OverlayPage } from "./OverlayPage";
import { PatchAssignmentView } from "./PatchAssignmentView";
import type { StreamingCompareState } from "./useCompareStream";

/** Full-screen result viewer for `GET /download_queue/{id}/compare/stream` — its own full-viewport
 * overlay (not the shared `Modal`) since judging image quality needs near-real reading resolution.
 *
 * Opens the moment the caller has a first sample (`state.samples` has at least one defined entry —
 * see `QueueItemRow`'s own `hasFirstSample`), before the rest of the comparison (`state.summary`)
 * has necessarily arrived — issue #77's own confirmed streaming design: the modal shows what it has
 * and fills in the header/recommendation area once `summary` lands, rather than blocking the whole
 * view on it. `state.samples` is a sparse array (indices without a value yet render as loading
 * placeholders) that's mutated in place as `"precise"` refinement events replace earlier `"coarse"`
 * ones — `OverlayPage` re-renders automatically off its own `sample` prop identity changing.
 *
 * Three shapes once `summary` is available: `likely_different_language` (no A/B pick, just a
 * notice), `recommendation` present (a pick + supporting samples), or neither (samples only, user
 * decides).
 *
 * "Keep A" overwrites B with the freshly-downloaded A; "Keep B" discards A and leaves B untouched.
 * Either one with unmatched pages on the other side opens `PatchAssignmentView` first to choose
 * which extra pages to carry over as a patch — both actions are disabled until `summary` (which
 * carries `a_unmatched_pages`/`b_unmatched_pages`) has arrived. */
export function ComparisonResultModal({
  queueItemId,
  state,
  onClose,
}: {
  queueItemId: string;
  state: StreamingCompareState;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const overwrite = useOverwriteQueueItem();
  const keepSideB = useKeepSideB();
  const [assigning, setAssigning] = useState<"keep-a" | "keep-b" | null>(null);
  const narrow = useIsNarrowViewport();
  const { summary, samples: rawSamples } = state;
  const samples = rawSamples.filter((s) => s !== undefined);
  const [touchMagnifyStore] = useState(() => createTouchMagnifyStore());

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && !assigning) onClose();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, assigning]);

  // This modal (and `PatchAssignmentView`, an internal state swap of the same mount, not a
  // separate one) is `position: fixed`, not portaled — the underlying `UploadPage` queue list is
  // still in the document and still scrollable on its own, so the browser's own page-level
  // scrollbar and scroll position stay live behind the modal unless explicitly locked here
  // (reported live, screenshotted: outer scrollbar + bottom button row cut off, which the modal's
  // own internal `overflow: hidden`/fixed height can't fix since that scrollbar belongs to `body`, not
  // this component's own box). Same pattern as `Reader.tsx`'s boundary-overlay lock.
  useEffect(() => {
    const prevOverflow = document.body.style.overflow;
    const prevOverscroll = document.body.style.overscrollBehavior;
    document.body.style.overflow = "hidden";
    document.body.style.overscrollBehavior = "none";
    return () => {
      document.body.style.overflow = prevOverflow;
      document.body.style.overscrollBehavior = prevOverscroll;
    };
  }, []);

  async function finishKeepA(insertions: ExportPatchInsertion[]) {
    setAssigning(null);
    try {
      await overwrite.mutateAsync({ id: queueItemId, insertions });
      onClose();
    } catch {
      toast({
        heading:
          t("upload.failedToResolveTheConflict") ??
          "Failed to resolve the conflict",
        icon: "error",
      });
    }
  }

  async function finishKeepB(insertions: ExportPatchInsertion[]) {
    setAssigning(null);
    try {
      await keepSideB.mutateAsync({ id: queueItemId, insertions });
      onClose();
    } catch {
      toast({
        heading:
          t("upload.failedToResolveTheConflict") ??
          "Failed to resolve the conflict",
        icon: "error",
      });
    }
  }

  function handleKeepA() {
    if (!summary) return;
    if (summary.b_unmatched_pages.length > 0) {
      setAssigning("keep-a");
    } else {
      void finishKeepA([]);
    }
  }

  function handleKeepB() {
    if (!summary) return;
    if (summary.a_unmatched_pages.length > 0) {
      setAssigning("keep-b");
    } else {
      void finishKeepB([]);
    }
  }

  if (assigning === "keep-a" && summary) {
    return (
      <PatchAssignmentView
        queueItemId={queueItemId}
        sourceSide="b"
        targetSide="a"
        sourceTotalPages={summary.b_total_pages}
        unmatchedPages={summary.b_unmatched_pages}
        onConfirm={(insertions) => void finishKeepA(insertions)}
        onCancel={() => setAssigning(null)}
      />
    );
  }
  if (assigning === "keep-b" && summary) {
    return (
      <PatchAssignmentView
        queueItemId={queueItemId}
        sourceSide="a"
        targetSide="b"
        sourceTotalPages={summary.a_total_pages}
        unmatchedPages={summary.a_unmatched_pages}
        onConfirm={(insertions) => void finishKeepB(insertions)}
        onCancel={() => setAssigning(null)}
      />
    );
  }

  const pending = overwrite.isPending || keepSideB.isPending;

  return (
    <TouchMagnifyStoreContext.Provider value={touchMagnifyStore}>
    <div
      style={{
        position: "fixed",
        // `inset: 0` (spelled out, not the shorthand, so the comment below has somewhere to live)
        // — no `height`, no `dvh`/`svh`, no JS viewport tracking. Across Blink/WebKit/Gecko, a
        // `position: fixed` element's containing block is specifically the LAYOUT viewport, which
        // itself already tracks the toolbar-driven dynamic viewport size natively — `top: 0` and
        // `bottom: 0` on a fixed element resolve against that automatically, no matter how the
        // toolbar animates, with no extra work. Every earlier attempt this session (`dvh`, `svh`,
        // `visualViewport`-driven JS height, `window.innerHeight`-driven JS height) was working
        // around a problem this specific element type doesn't actually have — the real bug this
        // whole time was the missing `overflow: hidden`/`boxSizing: border-box` below (both still
        // needed, and already confirmed fixed via `getBoundingClientRect` measurement).
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        // A hard clip boundary — without it, any child's natural height exceeding the viewport
        // budget grows the box itself past the viewport instead of the inner `overflowY: "auto"`
        // grid shrinking to fit, producing a page-level scrollbar and pushing the bottom Keep A/B
        // row off-screen (reported live, screenshotted: outer page scrollbar + cut-off buttons).
        overflow: "hidden",
        // Mobile Firefox elastic overscroll leaks white edges around the viewport — suppress.
        overscrollBehavior: "none",
        // The default `content-box` sizing adds `padding` on top of the declared height instead of
        // inside it — measured live via `getBoundingClientRect`: a 905px box rendered 937px tall
        // (exactly `905 + 16*2` padding), 32px past the viewport, with the excess silently clipped
        // by `overflow: hidden` from the bottom — this, not the viewport-height source or the
        // missing body-scroll-lock (both real fixes too, but neither was this bug), was the actual
        // mechanism cutting off the Keep A/B row. `border-box` folds padding inside the declared
        // height so the box never exceeds its budget in the first place.
        boxSizing: "border-box",
        zIndex: 9500,
        background: "rgba(0,0,0,0.92)",
        display: "flex",
        flexDirection: "column",
        padding: 16,
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "flex-start",
          justifyContent: "space-between",
          color: "#fff",
        }}
      >
        <div>
          <h3 style={{ margin: 0 }}>{t("upload.aiQualityComparison")}</h3>
          {summary ? (
            <>
              <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, marginTop: 4 }}>
                {t("upload.ofABPagesMatched", {
                  aligned: summary.aligned_pairs,
                  a: summary.a_total_pages,
                  b: summary.b_total_pages,
                })}
              </div>
              <div
                style={{
                  fontSize: FONT_SIZE_SM,
                  opacity: 0.8,
                  marginTop: 2,
                  display: "flex",
                  alignItems: "center",
                }}
              >
                {t("upload.aNameSize", {
                  name: summary.a_filename,
                  size: formatBytes(summary.a_file_size),
                })}
                <EntryTreePopover entries={summary.a_entries} />
              </div>
              <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, display: "flex", alignItems: "center" }}>
                {t("upload.bNameSize", {
                  name: summary.b_filename,
                  size: formatBytes(summary.b_file_size),
                })}
                <EntryTreePopover entries={summary.b_entries} />
              </div>
            </>
          ) : (
            <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, marginTop: 4 }}>
              <i className="fa fa-spinner fa-spin" aria-hidden="true" style={{ marginRight: 4 }}></i>
              {t("upload.stillComparingTheRestOf")}
            </div>
          )}
        </div>
        <IconButton
          icon="fa fa-times"
          onClick={onClose}
          size={32}
          className="modal-close-btn"
          // `touch-action: manipulation` — reported live: "modal右上角关闭按钮点击后延迟响应". This
          // button sits directly above the scrollable sample grid below it (`overflowY: "auto"`);
          // a tap landing right after the user's finger was scrolling/panning that grid can fall
          // into the browser's own post-scroll tap-disambiguation window (most pronounced on iOS
          // Safari) without this, since the default `touch-action: auto` leaves room for the
          // browser to still be deciding whether this new touch is another pan gesture.
          style={{ borderRadius: "50%", touchAction: "manipulation" }}
        />
      </div>

      {summary && (
        <div style={{ color: "#fff", marginTop: 8 }}>
          {summary.likely_different_language ? (
            <div style={{ padding: "4px 0" }}>
              <p style={{ margin: 0 }}>
                {t(
                  "upload.theseTwoFilesAlignAlmost",
                )}
              </p>
            </div>
          ) : summary.recommendation ? (
            <div style={{ padding: "4px 0", fontWeight: 700 }}>
              {t("upload.suggestionKeepVersionSide", {
                side: summary.recommendation === "a" ? "A" : "B",
              })}
            </div>
          ) : (
            <div style={{ padding: "4px 0" }}>
              {t(
                "upload.notConfidentEnoughToSuggest",
              )}
            </div>
          )}
        </div>
      )}

      {samples.length > 0 && (
        <>
          <div
            style={{
              fontSize: FONT_SIZE_MD,
              color: "#fff",
              opacity: 0.75,
              marginTop: 12,
              textAlign: "center",
            }}
          >
            <i
              className="fa fa-info-circle"
              aria-hidden="true"
              style={{ marginRight: 4 }}
            ></i>
            {t(
              "upload.compareTheSamplePagesBelow",
            )}
          </div>
          <div
            style={{
              flex: 1,
              minHeight: 0,
              display: "grid",
              gridTemplateColumns: narrow
                ? "1fr"
                : `repeat(${Math.min(samples.length, 3)}, 1fr)`,
              gap: 16,
              marginTop: 8,
              overflowY: "auto",
              overscrollBehavior: "contain",
              paddingBottom: 8,
            }}
          >
            {samples.map((sample) => (
              <OverlayPage
                key={`${sample.a_page_index}-${sample.b_page_index}`}
                queueItemId={queueItemId}
                sample={sample}
              />
            ))}
          </div>
        </>
      )}

      <div
        style={{
          display: "flex",
          justifyContent: "flex-end",
          gap: 8,
          marginTop: 12,
        }}
      >
        <KeepSideButton
          side="a"
          recommended={summary?.recommendation === "a"}
          onClick={handleKeepA}
          disabled={pending || !summary}
        />
        <KeepSideButton
          side="b"
          recommended={summary?.recommendation === "b"}
          onClick={handleKeepB}
          disabled={pending || !summary}
        />
      </div>
      <TouchMagnifyToggleButton />
    </div>
    </TouchMagnifyStoreContext.Provider>
  );
}

// The single fixed A/B toggle for whichever sample currently owns a touch-magnify session.
function TouchMagnifyToggleButton() {
  const { t } = useTranslation();
  const active = useTouchMagnifyStore((s) => s.active);
  if (!active) return null;
  return (
    <button
      type="button"
      onTouchStart={(e) => {
        e.stopPropagation();
        active.toggleSide();
      }}
      aria-label={t("upload.switchToTheOtherVersion") ?? undefined}
      style={{
        position: "fixed",
        // 90, not 24 — the modal's own bottom Keep A/B button row (plus its recommendation bubble
        // above a recommended one) sits in that lower strip; reported live as covered by this
        // button at the smaller offset.
        bottom: 90,
        left: active.toggleOnRight ? undefined : 24,
        right: active.toggleOnRight ? 24 : undefined,
        zIndex: 9800,
        width: 44,
        height: 44,
        borderRadius: "50%",
        border: "2px solid rgba(255,255,255,0.85)",
        background: active.showB ? "rgba(46,125,79,0.9)" : "rgba(58,124,199,0.9)",
        color: "#fff",
        fontSize: FONT_SIZE_SM,
        fontWeight: 700,
      }}
    >
      {active.showB ? "B" : "A"}
    </button>
  );
}
