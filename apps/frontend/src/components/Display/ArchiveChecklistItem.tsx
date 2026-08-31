/** One `<li>` row in a checklist of archives — a checkbox + title, toggled on click. Uses
 * `<label>`-wraps-`<input>` (not `id`+`htmlFor`) so callers needn't worry about `id` collisions. */
export function ArchiveChecklistItem({
  title,
  checked,
  onChange,
  className,
}: {
  title: React.ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  className?: string
}) {
  return (
    <li className={className}>
      <label>
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange(e.target.checked)}
          // Zeroes the UA default checkbox margin and aligns it to the title text's vertical center.
          style={{ margin: 0, verticalAlign: "middle" }}
        />{" "}
        <span>{title}</span>
      </label>
    </li>
  )
}
