/** One `<li>` row in a checklist of archives — the shared shape between `Categories.tsx`'s
 * static-category archive list and `Batch.tsx`'s selection list: a checkbox + the archive's title,
 * toggled on click. Uses the `<label>`-wraps-`<input>` association (rather than `id`+`htmlFor`) so
 * neither caller needs to worry about `id` collisions when the same archive could in principle
 * appear in more than one checklist-rendering context on the page at once. */
export default function ArchiveChecklistItem({
  title,
  checked,
  onChange,
}: {
  title: string
  checked: boolean
  onChange: (checked: boolean) => void
}) {
  return (
    <li>
      <label>
        <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} /> {title}
      </label>
    </li>
  )
}
