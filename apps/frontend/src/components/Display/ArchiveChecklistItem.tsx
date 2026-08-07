/** One `<li>` row in a checklist of archives — the shared shape between `Categories.tsx`'s
 * static-category archive list and `Batch.tsx`'s selection list: a checkbox + the archive's title,
 * toggled on click. Uses the `<label>`-wraps-`<input>` association (rather than `id`+`htmlFor`) so
 * neither caller needs to worry about `id` collisions when the same archive could in principle
 * appear in more than one checklist-rendering context on the page at once.
 *
 * `title` accepts any node, not just a plain string — `Categories.tsx` wraps it in a `Tooltip` for
 * an archive that's also a Tankoubon member, and widening this shared prop's type is simpler than
 * either duplicating this row's markup there or teaching this generic component about Tankoubons
 * specifically. A plain `string` (every other caller's usage today) still satisfies `ReactNode`.
 *
 * `className` (optional, on the `<li>` itself) is the same kind of generic escape hatch — used by
 * `Categories.tsx` to give a Tankoubon-member row a highlighted background via a real per-theme
 * CSS class (`.tankoubon-member-row`, defined once per theme file — see those files' own docs for
 * why a class beats a hardcoded color here), without this component needing to know that's why. */
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
          // Browser UA default checkbox margin/vertical-align (`margin: 3px 3px 3px 4px`,
          // `vertical-align: baseline` — confirmed live via `getComputedStyle`) leaves the
          // checkbox visibly offset from the adjacent title text's own baseline; no theme CSS
          // resets this for a bare `input[type=checkbox]`. `verticalAlign: "middle"` aligns it to
          // the text's vertical center instead, and zeroing the margin removes the asymmetric gap
          // that stacked on top of the baseline mismatch.
          style={{ margin: 0, verticalAlign: "middle" }}
        />{" "}
        {/* Real wrapper, not a bare text node — lets a caller turn this row into a two-column
         * flex layout (checkbox column + title column) via CSS alone, keeping a long title's own
         * wrapped lines aligned under its own first line instead of back at the checkbox's left
         * edge (AiSmartTankoubonModal.tsx's `.ai-group-checklist-item` does this). */}
        <span>{title}</span>
      </label>
    </li>
  )
}
