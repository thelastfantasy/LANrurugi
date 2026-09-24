export function FilterChip({
  active,
  label,
  count,
  color,
  onClick,
}: {
  active: boolean
  label: string
  count: number
  color?: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className="stdbtn"
      onClick={onClick}
      style={{
        fontWeight: active ? "bold" : "normal",
        outline: active ? "2px solid currentColor" : "none",
        color: color ?? "inherit",
      }}
    >
      {label} ({count})
    </button>
  )
}
