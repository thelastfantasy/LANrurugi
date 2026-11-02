// CSS-sprite-clip star row (empty-star base + gold-star image clipped to the rated fraction),
// not Font Awesome's fixed icons — needed for fractional stars at 0.1 precision.

const STAR_SIZE = 24
const MAX_STARS = 5

/** One star's own fill fraction (0-1) given the overall `rating` and this star's 0-based index —
 * star 0 is fully gold once `rating >= 1`, partially gold while `0 < rating < 1`, empty at `0`. */
function starFraction(rating: number, index: number): number {
  return Math.max(0, Math.min(1, rating - index))
}

/** Read-only star row for a 0-5 rating at tenth precision. `RatingWidget.tsx` reuses this
 * rendering but adds click/hover handling for the interactive case. */
export function StarRatingDisplay({
  rating,
  size = STAR_SIZE,
}: {
  rating: number
  size?: number
}) {
  return (
    <span style={{ display: "inline-flex" }}>
      {Array.from({ length: MAX_STARS }, (_, i) => (
        <StarSprite key={i} fraction={starFraction(rating, i)} size={size} />
      ))}
    </span>
  )
}

/** One star cell: empty-star base layer, gold-star image cropped to `fraction` width on top via
 * `overflow: hidden` on the wrapper (the gold `<img>` itself always renders at full `size`). */
export function StarSprite({ fraction, size }: { fraction: number; size: number }) {
  return (
    <span style={{ position: "relative", display: "inline-block", width: size, height: size }}>
      <img
        src="/legacy/img/star-empty.svg"
        alt=""
        draggable={false}
        style={{ position: "absolute", top: 0, left: 0, width: size, height: size }}
      />
      {fraction > 0 && (
        <span
          style={{
            position: "absolute",
            top: 0,
            left: 0,
            width: `${fraction * 100}%`,
            height: size,
            overflow: "hidden",
          }}
        >
          <img
            src="/legacy/img/star-full.svg"
            alt=""
            draggable={false}
            style={{ width: size, height: size, display: "block" }}
          />
        </span>
      )}
    </span>
  )
}
