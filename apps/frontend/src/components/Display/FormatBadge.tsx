import type { CSSProperties } from "react"

import { isBookExtension } from "@/lib/utils/bookFormats"

/** File format of an e-book archive (PDF, EPUB, …), shown beside its title; nothing for ordinary
 *  archives. Drawn in the surrounding text colour so it fits every theme. */
export function FormatBadge({
  extension,
  style,
}: {
  extension: string | null | undefined
  style?: CSSProperties
}) {
  if (!isBookExtension(extension)) return null
  return (
    <span
      style={{
        display: "inline-block",
        flexShrink: 0,
        border: "1px solid currentColor",
        borderRadius: 3,
        padding: "0 3px",
        fontSize: "0.8em",
        fontWeight: "bold",
        lineHeight: 1.3,
        letterSpacing: "0.02em",
        opacity: 0.75,
        verticalAlign: "middle",
        whiteSpace: "nowrap",
        ...style,
      }}
    >
      {extension?.toUpperCase()}
    </span>
  )
}
