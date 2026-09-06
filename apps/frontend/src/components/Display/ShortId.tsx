/** Visually clips a UUID to its first 7 chars, but the full id stays as real DOM text — clicking
 *  selects the whole value for copying, and `title` shows it on hover. */
export function ShortId({ id }: { id: string }) {
  function selectAll(e: React.MouseEvent<HTMLSpanElement>) {
    const selection = window.getSelection()
    if (!selection) return
    const range = document.createRange()
    range.selectNodeContents(e.currentTarget)
    selection.removeAllRanges()
    selection.addRange(range)
  }
  return (
    <span
      title={id}
      onClick={selectAll}
      style={{ display: "inline-block", width: "7ch", overflow: "hidden", cursor: "text" }}
    >
      {id}
    </span>
  )
}
