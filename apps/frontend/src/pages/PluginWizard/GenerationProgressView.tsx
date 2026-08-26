import { useTranslation } from "react-i18next"

import type { ProgressItem } from "./useGenerationProgress"

/** Shared render for `useGenerationProgress`'s accumulated state — a live elapsed-time counter,
 * then `items` in the order they actually happened (a `fetch_page`/`fetch_result` log line
 * interleaved with the model's own streamed commentary/answer at the point it arrived, not two
 * separate blocks). Renders nothing once there's neither an item nor any elapsed time yet (before
 * the first tick/SSE event, or after a `start()`/before it), so it never shows an empty box. */
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
