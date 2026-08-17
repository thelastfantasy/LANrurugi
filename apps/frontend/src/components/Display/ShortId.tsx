/** The full UUID (36 chars) took too much width for a column that's mostly useful for "confirm
 *  which row this is" rather than actually being read character-by-character — visually clipped to
 *  its first 7 characters (matching `git`'s own default short-hash length, a familiar convention
 *  for "enough to usually disambiguate, not the whole thing") via `overflow: hidden` on a
 *  fixed-width container, rather than actually truncating the *text*: the full id is still the
 *  real DOM text content underneath, so a click selecting "everything in this element" (the handler
 *  below) selects the complete value ready to copy, not just whatever few characters happen to be
 *  visible. The native `title` hover tooltip also still shows the full value without needing to
 *  click first. Shared by `Settings/ApiTokensSection.tsx` and the Activity page — both render
 *  UUID-shaped ids in a tight table column. */
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
