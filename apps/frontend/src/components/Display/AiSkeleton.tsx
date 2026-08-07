import { useEffect, useState } from "react"

/** Modern AI-thinking skeleton — pulsing robot icon + shimmer bars + animated dots. Extracted
 * from `TankoubonEdit.tsx` (originally private to that page's own AI-rename overlay) so
 * `Categories.tsx`'s AI-grouping-suggestions modal can reuse the exact same loading treatment
 * instead of re-implementing it — every caller gets the same "AI is thinking" feel regardless of
 * which AI feature is actually running. Ships its own `@keyframes`/`.ai-skel-bar` styles inline
 * (via a one-off `<style>` tag) so a caller doesn't need to separately import CSS — safe to mount
 * more than once on the same page, since repeated identical `@keyframes` declarations are a no-op
 * in CSS, not an error. */
export function AiSkeleton() {
  const [dots, setDots] = useState(0)
  useEffect(() => {
    const t = setInterval(() => setDots((d) => (d + 1) % 4), 400)
    return () => clearInterval(t)
  }, [])
  return (
    <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 16, padding: "20px 0" }}>
      {/* Pulsing robot icon */}
      <div style={{ animation: "ai-pulse 1.5s ease-in-out infinite" }}>
        <i className="fa fa-robot" aria-hidden="true" style={{ fontSize: 40, opacity: 0.7 }} />
      </div>
      <div style={{ fontSize: 14, opacity: 0.6, fontFamily: "monospace" }}>
        {"AI 正在思考" + ".".repeat(dots)}
      </div>
      {/* Shimmer bars — hint at content without fake cards */}
      <div style={{ width: "100%", display: "flex", flexDirection: "column", gap: 10, padding: "0 8px" }}>
        <div className="ai-skel-bar" style={{ width: "70%" }} />
        <div className="ai-skel-bar" style={{ width: "50%" }} />
        <div className="ai-skel-bar" style={{ width: "60%" }} />
        <div className="ai-skel-bar" style={{ width: "40%" }} />
      </div>
      <style>{`
        @keyframes ai-shimmer {
          0%   { background-position: -200% 0; }
          100% { background-position: 200% 0; }
        }
        @keyframes ai-pulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.4; }
        }
        .ai-skel-bar {
          height: 12px;
          border-radius: 4px;
          background: linear-gradient(90deg, rgba(128,128,128,0.08) 25%, rgba(128,128,128,0.2) 50%, rgba(128,128,128,0.08) 75%);
          background-size: 200% 100%;
          animation: ai-shimmer 1.8s ease-in-out infinite;
        }
      `}</style>
    </div>
  )
}
