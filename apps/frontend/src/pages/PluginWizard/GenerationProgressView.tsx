import { useTranslation } from "react-i18next"

import type { ProgressItem } from "./useGenerationProgress"

/** Renders `useGenerationProgress`'s accumulated items in arrival order, plus a live elapsed-time counter.
 * Renders nothing when there's neither an item nor elapsed time yet. */
export function GenerationProgressView({ items, elapsedSeconds }: { items: ProgressItem[]; elapsedSeconds: number }) {
  const { t } = useTranslation()
  if (items.length === 0 && elapsedSeconds === 0) return null
  return (
    <div
      className="ptbox"
      style={{
        padding: 8,
        marginTop: 4,
        maxHeight: 200,
        overflowY: "auto",
        fontFamily: "monospace",
        fontSize: "9pt",
      }}
    >
      <div style={{ fontWeight: "bold" }}>{t("pluginWizard.streamElapsed", { seconds: elapsedSeconds })}</div>
      {items.map((item, i) =>
        item.kind === "log" ? (
          <div key={i}>{item.text}</div>
        ) : (
          <pre key={i} style={{ whiteSpace: "pre-wrap", margin: 0 }}>
            {item.text}
          </pre>
        ),
      )}
    </div>
  )
}
