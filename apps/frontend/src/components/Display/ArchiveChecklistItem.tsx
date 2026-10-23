import { CheckboxField } from "@/components/common-ui/Form"

/** One `<li>` row in a checklist of archives — a checkbox + title, toggled on click.
 * `trailing` renders outside `CheckboxField`'s own `<label>` (its own click doesn't toggle the
 * checkbox) — for a row-level action like a read button. */
export function ArchiveChecklistItem({
  title,
  checked,
  onChange,
  className,
  trailing,
}: {
  title: React.ReactNode
  checked: boolean
  onChange: (checked: boolean) => void
  className?: string
  trailing?: React.ReactNode
}) {
  return (
    <li
      className={className}
      // Only flips to a flex row when `trailing` is actually present — the default (no prop)
      // stays plain block flow so `.checklist li`'s own `white-space: nowrap`/`text-overflow:
      // ellipsis` (public/legacy/lrr.css) keeps truncating a long title the way it always has for
      // `Batch.tsx`/`Categories.tsx`'s own calls, neither of which pass `trailing`.
      style={trailing ? { display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 } : undefined}
    >
      <span style={trailing ? { minWidth: 0, flex: 1 } : undefined}>
        <CheckboxField checked={checked} onCheckedChange={onChange} fill={!!trailing}>
          {/* With `trailing`, `fill` lets the label shrink and this span carries `.checklist li`'s
              truncation itself. `data-truncate-box` lets a title measure whether it was cut off. */}
          <span
            data-truncate-box=""
            style={trailing ? { display: "block", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" } : undefined}
          >
            {title}
          </span>
        </CheckboxField>
      </span>
      {trailing}
    </li>
  )
}
