import type { ReactNode } from "react"

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

/** Wraps every case-insensitive match of any space-separated keyword in `query` with `<mark>`.
 * Returns `text` unchanged (as a plain string) when `query` is empty/whitespace-only. */
export function highlightText(text: string, query: string | undefined): ReactNode {
  const keywords = (query ?? "")
    .split(/\s+/)
    .map((k) => k.trim())
    .filter(Boolean)
  if (keywords.length === 0) return text

  const pattern = new RegExp(`(${keywords.map(escapeRegExp).join("|")})`, "gi")
  const parts = text.split(pattern)
  if (parts.length === 1) return text

  return parts.map((part, i) =>
    i % 2 === 1 ? (
      // Fixed dark text, not `color: inherit` — this renders inside containers with their own
      // light or dark text color (e.g. a white-on-dark bookmark-name overlay), and white text on
      // a light yellow highlight is nearly unreadable.
      <mark key={i} style={{ background: "#fff59d", color: "#3a3000" }}>
        {part}
      </mark>
    ) : (
      part
    ),
  )
}
