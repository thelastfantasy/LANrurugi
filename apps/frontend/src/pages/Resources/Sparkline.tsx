/** A sparkline for one series. No axis, no labels, no hover: the numeric row beside it carries the
 * reading, and the shape answers the only question a 30-minute curve is asked ("is it climbing?").
 *
 * Rendered as an SVG path so it costs one element per point rather than a canvas that has to be sized
 * and repainted. A series with a single value draws a flat line at that value — a lone point at 0
 * height would read as "idle". */
export function Sparkline({
  values,
  width = 120,
  height = 28,
  className,
  label,
}: {
  values: number[]
  width?: number
  height?: number
  className?: string
  /** Accessible name; the caller knows what the series is. */
  label: string
}) {
  if (values.length === 0) {
    return <span className="resources-sparkline-empty" style={{ width, height }} aria-label={label} />
  }

  const padding = 1.5
  const min = Math.min(...values)
  const max = Math.max(...values)
  const span = max - min
  const usable = height - padding * 2
  const step = values.length > 1 ? width / (values.length - 1) : 0

  const points = values.map((value, index) => {
    const x = values.length > 1 ? index * step : width / 2
    // A flat series sits mid-height rather than on the floor; a real range is scaled to fill.
    const ratio = span > 0 ? (value - min) / span : 0.5
    const y = padding + usable * (1 - ratio)
    return `${x.toFixed(2)},${y.toFixed(2)}`
  })

  return (
    <svg
      className={className ? `resources-sparkline ${className}` : "resources-sparkline"}
      width={width}
      height={height}
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={label}
      preserveAspectRatio="none"
    >
      <polyline points={points.join(" ")} fill="none" stroke="currentColor"
        strokeWidth={1.5} vectorEffect="non-scaling-stroke" />
    </svg>
  )
}
