import { useTranslation } from "react-i18next";

import type { ComparisonSide } from "@/api/types";
import { useMenuPalette } from "@/hooks/useMenuPalette";
import { FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme";

/** Always-visible speech-bubble callout above a recommended button, arrow pointing down at it. */
function RecommendationBubble() {
  const { t } = useTranslation();
  const palette = useMenuPalette();
  return (
    <div
      style={{
        position: "absolute",
        bottom: "100%",
        left: "50%",
        transform: "translateX(-50%)",
        marginBottom: 8,
        whiteSpace: "nowrap",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 6,
          padding: "6px 10px",
          borderRadius: 6,
          background: palette.bg,
          border: `1px solid ${palette.border}`,
          color: palette.text,
          fontSize: FONT_SIZE_XS,
          boxShadow: palette.shadow,
        }}
      >
        <i className="fa fa-robot" aria-hidden="true"></i>
        {t("AI recommends keeping this version")}
      </div>
      {/* Two-layer triangle: a slightly larger bordered triangle underneath simulates the arrow's
          own outline (matching the bubble's `border`), a same-color triangle 1px inset on top
          covers everything but that outline sliver — without this the arrow only ever showed its
          fill color with no border, an obvious seam against the bubble's own bordered edge.
          Parked near the bubble's own left edge (not centered) — a dead-center arrow read as
          visually flat/uninteresting. */}
      <div style={{ position: "relative", width: 14, height: 7, marginTop: -1, marginLeft: 14 }}>
        <div
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: 0,
            height: 0,
            borderLeft: "7px solid transparent",
            borderRight: "7px solid transparent",
            borderTop: `7px solid ${palette.border}`,
          }}
        ></div>
        <div
          style={{
            position: "absolute",
            top: -1,
            left: 1,
            width: 0,
            height: 0,
            borderLeft: "6px solid transparent",
            borderRight: "6px solid transparent",
            borderTop: `6px solid ${palette.bg}`,
          }}
        ></div>
      </div>
    </div>
  );
}

export function KeepSideButton({
  side,
  recommended,
  onClick,
  disabled,
}: {
  side: ComparisonSide;
  recommended: boolean;
  onClick: () => void;
  disabled: boolean;
}) {
  const { t } = useTranslation();
  return (
    <div style={{ position: "relative" }}>
      {recommended && <RecommendationBubble />}
      <button
        type="button"
        className="stdbtn"
        onClick={onClick}
        disabled={disabled}
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          height: 40,
          padding: "0 20px",
          fontSize: FONT_SIZE_SM,
        }}
      >
        {t("Keep version {{side}}", { side: side === "a" ? "A" : "B" })}
      </button>
    </div>
  );
}
