import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

import { useKeepSideB, useOverwriteQueueItem } from "@/api/hooks";
import type { ExportPatchInsertion } from "@/api/types";
import { IconButton } from "@/components/common-ui/Form"
import { formatBytes } from "@/components/Display"
import { useIsNarrowViewport } from "@/hooks";
import { createTouchMagnifyStore, TouchMagnifyStoreContext, useTouchMagnifyStore } from "@/store";
import { FONT_SIZE_MD, FONT_SIZE_SM } from "@/theme";
import { toast } from "@/toast";

import { EntryTreePopover } from "./EntryTreePopover";
import { KeepSideButton } from "./KeepSideButton";
import { OverlayPage } from "./OverlayPage";
import { PatchAssignmentView } from "./PatchAssignmentView";
import type { StreamingCompareState } from "./useCompareStream";

/** Full-screen result viewer for the compare stream — own full-viewport overlay, not the shared
 * `Modal`. "Keep A/B" opens `PatchAssignmentView` first if the other side has unmatched pages. */
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
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        // Hard clip boundary — without it, a child's natural height grows the box itself past the
        // viewport instead of the inner overflowY: auto grid shrinking to fit.
        overflow: "hidden",
        overscrollBehavior: "none",
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

/** The single fixed A/B toggle for whichever sample currently owns a touch-magnify session. */
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
        // 90, not 24 — the modal's bottom Keep A/B button row sits in that lower strip.
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
