import type { SubscriptionSort, SubscriptionSortKey } from "./subscriptionVerdicts"

/** A column heading that orders the list under it.
 *
 * Keeps the caller's own `preview-*` class — those classes are what the grid's column tracks are
 * measured from — and adds the button reset. A real `<button>` is worth the restyling: keyboard
 * focus, Enter/Space and the pointer cursor come with it. */
export function SortHead({
  className,
  label,
  sortKey,
  sort,
  onSort,
}: {
  className: string
  label: string
  sortKey: SubscriptionSortKey
  sort: SubscriptionSort
  onSort: (key: SubscriptionSortKey) => void
}) {
  const active = sort.key === sortKey
  return (
    <button
      type="button"
      className={`${className} preview-head-sort${active ? " preview-head-sort-active" : ""}`}
      onClick={() => onSort(sortKey)}
      // The caret is a shape, not a name: "pressed" is what tells a screen reader this is the order
      // in use, and the heading text is already the button's own label.
      aria-pressed={active}
      title={label}
    >
      <span className="preview-head-sort-label">{label}</span>
      <i
        className={`fa ${active ? (sort.dir === "asc" ? "fa-caret-up" : "fa-caret-down") : "fa-sort"}`}
        aria-hidden="true"
      />
    </button>
  )
}
