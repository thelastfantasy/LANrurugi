// Shared star-rating rendering — a CSS-sprite-clip star row (empty-star background image +
// gold-star image, the gold layer's width clipped to the exact rated fraction) rather than
// Font Awesome's fixed on/off star icons, since only a continuously-clippable image (not a
// discrete icon glyph) can render fractional stars at 0.1 precision — e.g. a rating of 4.3 shows
// the 5th star's gold layer clipped to 30% width, not just rounded to the nearest half/whole
// icon. Modeled on E-Hentai's own rating widget (empty-star + colored-star image pair, clipped by
// width), the only real prior art for this exact interaction the user pointed to directly.

const STAR_SIZE = 24
const MAX_STARS = 5

/** One star's own fill fraction (0-1) given the overall `rating` and this star's 0-based index —
 * star 0 is fully gold once `rating >= 1`, partially gold while `0 < rating < 1`, empty at `0`. */
function starFraction(rating: number, index: number): number {
  return Math.max(0, Math.min(1, rating - index))
}

/** Read-only star row for a 0-5 rating, clipped to a real tenth's precision — used wherever a
 * rating is *displayed*, not set (`ArchiveOverviewOverlay.tsx`'s `TagsTable`). The interactive
 * click/hover-to-set widget is `RatingWidget.tsx`, which reuses this same per-star sprite
 * rendering but adds pointer handling on top. */
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

/** One star cell: the empty-star image as the base layer, a gold-star image cropped to `fraction`
 * of its own width absolutely positioned on top — `overflow: hidden` on the crop wrapper is what
 * actually clips the gold layer's visible extent, not the image's own dimensions (the gold `<img>`
 * itself is always rendered at full `size`; only its containing box is narrower). */
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
