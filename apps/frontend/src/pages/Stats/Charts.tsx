import type { ReactElement, ReactNode } from "react"
import { ResponsiveContainer } from "recharts"

import { useMenuPalette } from "@/hooks/useMenuPalette"

/** Panel chrome for every `/stats` dashboard section: the theme's own popup palette (already the
 *  single source of truth for "a surface that is not a legacy panel" in this app — see
 *  `dialog.tsx`), so a card reads correctly on all five themes without a new colour literal. */
/** The height, in pixels, a non-zero bar is drawn at when its true height would be smaller.
 *
 *  A bucket holding one archive out of eighty is under a pixel tall: not clickable, and barely
 *  visible as a sliver. 8px is the point where such a band reads as a small bar rather than a
 *  rendering artifact, at the cost of overstating it (8px of a ~180px plot is what ~3.5 archives
 *  would draw) — the exact counts are in the table next to every chart, and a histogram whose small
 *  bands are invisible is worse than one that exaggerates them. Zero stays zero: no bar is drawn at
 *  all, so an empty bucket is visibly empty rather than a small lie. Shared by every bar chart on
 *  the page so they agree. */
export const MIN_BAR_PIXELS = 8

export function useStatsPanel() {
  const palette = useMenuPalette()
  return {
    palette,
    panel: {
      backgroundColor: palette.bg,
      border: `1px solid ${palette.separator}`,
      borderRadius: 8,
    } as const,
    muted: { opacity: 0.72 } as const,
  }
}

export function StatsSection({
  title,
  icon,
  hint,
  children,
}: {
  title: string
  icon: string
  hint?: string
  children: ReactNode
}) {
  const { panel } = useStatsPanel()
  return (
    <section style={{ ...panel, padding: "12px 14px", margin: "12px 0", textAlign: "left" }}>
      <h3 style={{ margin: "0 0 4px", fontSize: "1.1rem" }}>
        <i className={`fa ${icon}`} style={{ marginRight: 8 }} />
        {title}
      </h3>
      {hint ? (
        <p style={{ margin: "0 0 10px", fontSize: "0.75rem", opacity: 0.72 }}>{hint}</p>
      ) : null}
      {children}
    </section>
  )
}

/** One headline number with its label — the dashboard's equivalent of legacy stats' own counter
 *  lines, sized to sit in a wrapping grid. */
export function StatsCard({
  icon,
  label,
  value,
  hint,
}: {
  icon: string
  label: string
  value: ReactNode
  hint?: string
}) {
  const { panel } = useStatsPanel()
  return (
    // `boxSizing` is explicit because this page's legacy stylesheets leave the global default at
    // `content-box` (they load after Tailwind's preflight and override it). Without it the flex
    // basis below would exclude the card's own padding and border, so each card would need
    // 168+24+2 px of row and the grid would collapse to one card per line on a phone.
    <div style={{ ...panel, padding: "10px 12px", boxSizing: "border-box", minWidth: 168, flex: "1 1 168px" }}>
      <div style={{ fontSize: "0.75rem", opacity: 0.72, marginBottom: 4 }}>
        <i className={`fa ${icon}`} style={{ marginRight: 6 }} />
        {label}
      </div>
      <div style={{ fontSize: "1.35rem", lineHeight: 1.2 }}>{value}</div>
      {hint ? <div style={{ fontSize: "0.7rem", opacity: 0.62, marginTop: 4 }}>{hint}</div> : null}
    </div>
  )
}

/** A fixed-height responsive chart box. Recharts needs an explicitly sized parent; every chart
 *  here is inside one of these rather than measuring itself. */
export function ChartFrame({ height, children }: { height: number; children: ReactElement }) {
  return (
    <div style={{ width: "100%", height }}>
      <ResponsiveContainer width="100%" height="100%">
        {children}
      </ResponsiveContainer>
    </div>
  )
}

interface TooltipEntry {
  name?: string | number
  value?: string | number
  color?: string
  payload?: Record<string, unknown>
}

/** Recharts' default tooltip is a light-mode white box that ignores every theme; this one is
 *  built from the same palette as the panels, and only ever renders the entries the chart itself
 *  passed in (no invented series). */
export function ChartTooltip({
  active,
  payload,
  label,
  formatValue,
}: {
  active?: boolean
  payload?: TooltipEntry[]
  label?: string | number
  formatValue?: (value: string | number, entry: TooltipEntry) => string
}) {
  const { palette } = useStatsPanel()
  if (!active || !payload || payload.length === 0) return null
  return (
    <div
      style={{
        backgroundColor: palette.bg,
        border: `1px solid ${palette.separator}`,
        borderRadius: 4,
        color: palette.text,
        padding: "6px 8px",
        fontSize: "0.75rem",
        boxShadow: palette.shadow === "none" ? undefined : palette.shadow,
      }}
    >
      {label === undefined ? null : <div style={{ marginBottom: 4, opacity: 0.8 }}>{label}</div>}
      {payload.map((entry, index) => (
        <div key={`${entry.name}-${index}`} style={{ display: "flex", gap: 8, alignItems: "center" }}>
          <span
            aria-hidden
            style={{
              width: 8,
              height: 8,
              borderRadius: 2,
              backgroundColor: entry.color ?? "currentColor",
              display: "inline-block",
            }}
          />
          <span>{entry.name}</span>
          <span style={{ marginLeft: "auto" }}>
            {formatValue && entry.value !== undefined
              ? formatValue(entry.value, entry)
              : String(entry.value ?? "")}
          </span>
        </div>
      ))}
    </div>
  )
}

/** One row of a horizontal bar list: label, a bar proportional to `value / max`, and a value
 *  column. Used wherever a pie would need more colours than a task like "top tags" can justify. */
export function BarRow({
  label,
  value,
  max,
  valueLabel,
  color,
  hint,
  href,
  linkTitle,
}: {
  label: ReactNode
  value: number
  max: number
  valueLabel: ReactNode
  color: string
  hint?: string
  /** Where this row goes in the library (a search that reproduces exactly this row's set), if
   *  anywhere — the row's label becomes a link rather than a plain text run. */
  href?: string
  linkTitle?: string
}) {
  const width = max > 0 ? Math.max((value / max) * 100, value > 0 ? 1 : 0) : 0
  const labelContent = (
    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{label}</span>
  )
  return (
    <div style={{ display: "grid", gridTemplateColumns: "minmax(0,1fr) auto", gap: 8, marginBottom: 6 }}>
      <div style={{ minWidth: 0 }}>
        <div
          style={{
            display: "flex",
            justifyContent: "space-between",
            gap: 8,
            fontSize: "0.78rem",
            marginBottom: 2,
          }}
        >
          {href ? (
            <a href={href} title={linkTitle ?? href} style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
              {label}
            </a>
          ) : (
            labelContent
          )}
          {hint ? <span style={{ opacity: 0.65 }}>{hint}</span> : null}
        </div>
        <div style={{ backgroundColor: "rgba(127,127,127,0.22)", borderRadius: 3, height: 7 }}>
          <div style={{ width: `${width}%`, backgroundColor: color, height: "100%", borderRadius: 3 }} />
        </div>
      </div>
      <div style={{ alignSelf: "center", fontSize: "0.8rem", minWidth: 48, textAlign: "right" }}>
        {valueLabel}
      </div>
    </div>
  )
}

export function StatsTable({ head, rows }: { head: ReactNode[]; rows: ReactNode[][] }) {
  const { palette } = useStatsPanel()
  return (
    <div style={{ overflowX: "auto" }}>
      <table style={{ width: "100%", borderCollapse: "collapse", fontSize: "0.78rem" }}>
        <thead>
          <tr>
            {head.map((cell, index) => (
              <th
                key={index}
                style={{
                  textAlign: index === 0 ? "left" : "right",
                  borderBottom: `1px solid ${palette.separator}`,
                  padding: "4px 6px",
                  fontWeight: "normal",
                  opacity: 0.72,
                  whiteSpace: "nowrap",
                }}
              >
                {cell}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {rows.map((row, rowIndex) => (
            <tr key={rowIndex}>
              {row.map((cell, cellIndex) => (
                <td
                  key={cellIndex}
                  style={{
                    textAlign: cellIndex === 0 ? "left" : "right",
                    padding: "3px 6px",
                    borderBottom: `1px solid ${palette.separator}`,
                    whiteSpace: cellIndex === 0 ? "normal" : "nowrap",
                  }}
                >
                  {cell}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

/** A coloured swatch for an HTML legend next to a chart. */
export function LegendSwatch({ color }: { color: string }) {
  return (
    <span
      aria-hidden
      style={{
        width: 10,
        height: 10,
        borderRadius: 2,
        backgroundColor: color,
        display: "inline-block",
        marginRight: 6,
        flex: "0 0 auto",
      }}
    />
  )
}
