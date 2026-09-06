/** Date and time on their own lines — `toLocaleString()`'s combined string overflowed its grid
 * column at this table's font size; splitting avoids needing the column wide enough for one line. */
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
