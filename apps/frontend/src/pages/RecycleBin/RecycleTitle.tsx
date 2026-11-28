import type { CSSProperties, ReactNode } from "react"
import { createContext, useContext } from "react"

import { fullTitleOverlayStyle, useFullTitle } from "@/hooks/useFullTitle"

type FullTitle = ReturnType<typeof useFullTitle<HTMLDivElement>>

const FullTitleContext = createContext<FullTitle | null>(null)

const TITLE_LINE: CSSProperties = {
  fontWeight: "bold",
  maxWidth: "100%",
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
}

/** One list row. Hovering it (or pressing it on a phone) anywhere but its buttons unfolds the
 * row's title and path when either is cut off, the way a library card unfolds its title. */
export function RecycleRowRegion({ style, children }: { style?: CSSProperties; children: ReactNode }) {
  const full = useFullTitle<HTMLDivElement>("button")
  return (
    <FullTitleContext.Provider value={full}>
      <div style={style} {...full.handlers}>
        {children}
      </div>
    </FullTitleContext.Provider>
  )
}

/** The row's one-line title, with its one-line path beneath when there is one. */
export function RecycleTitle({ title, path }: { title: ReactNode; path?: string }) {
  const full = useContext(FullTitleContext)
  return (
    <div style={{ position: "relative", maxWidth: "100%", minWidth: 0 }}>
      <div ref={full?.titleRef} style={{ maxWidth: "100%" }}>
        <div style={TITLE_LINE} title={typeof title === "string" ? title : undefined}>
          {title}
        </div>
        {path && (
          <div className="recycle-path" style={{ maxWidth: "100%" }} title={path}>
            {path}
          </div>
        )}
      </div>
      {full?.overlay && (
        // Unlike the library card's, this copy is nothing to click through to: it takes the pointer
        // so the unfolded text can be selected and copied.
        <div aria-hidden="true" style={{ ...fullTitleOverlayStyle(full.overlay), pointerEvents: "auto", userSelect: "text" }}>
          <div style={{ fontWeight: "bold", overflowWrap: "anywhere" }}>{title}</div>
          {path && (
            <div className="recycle-path" style={{ whiteSpace: "normal", overflowWrap: "anywhere" }}>
              {path}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
