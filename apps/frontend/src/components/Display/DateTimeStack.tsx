/** Date and time on their own lines — `toLocaleString()`'s combined "2026/8/14 10:05:34" was
 *  overflowing its grid column at this table's own font size, since `whiteSpace: "nowrap"` (needed
 *  so the two halves don't wrap at an arbitrary space) left nothing shorter to wrap to. Splitting
 *  the value itself into two deliberate lines gives the same "never breaks mid-word" guarantee
 *  without needing the column to be wide enough for the full combined string in one line. Shared by
 *  `Settings/ApiTokensSection.tsx` and the Activity page — both render Unix-timestamp columns in a
 *  tight grid table. */
export function DateTimeStack({ epochSeconds }: { epochSeconds: number }) {
  const date = new Date(epochSeconds * 1000)
  return (
    <>
      {date.toLocaleDateString()}
      <br />
      {date.toLocaleTimeString()}
    </>
  )
}
