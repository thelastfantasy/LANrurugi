export function SortableHeaderLink({
  label,
  sortKey,
  onSort,
}: {
  label: string
  sortKey: string
  onSort: (key: string) => void
}) {
  return (
    <a
      href="#"
      onClick={(e) => {
        e.preventDefault()
        onSort(sortKey)
      }}
    >
      {label}
    </a>
  )
}
