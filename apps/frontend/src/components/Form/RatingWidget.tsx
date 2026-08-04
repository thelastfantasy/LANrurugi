import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateArchiveMetadata } from "@/api/hooks"
import { Tooltip } from "@/components/Overlay/Tooltip"
import { formatRating, parseRating } from "@/lib/utils/rating"

import { StarSprite } from "./StarRating"

// Mirrors legacy's Raty-based rating widget (`~/LANraragi/public/js/reader.js:315-337`) — there is
// no dedicated rating field or column anywhere in legacy's own data model; a rating is just a tag
// under the `rating:` namespace. Storage format is this app's own decimal encoding (`rating:4.5`),
// not legacy's `rating:⭐⭐⭐` star-repeat — see `lib/rating.ts`'s own docs for why (whole-number-only
// encoding can't represent a half star at all) and how old-format tags are still read correctly.
//
// Shared between the Reader overview overlay (its own persistence — a plain archive, always) and
// the Library page's right-click context menu, which needs its own `onChange` override since a
// menu target can be a tankoubon (`PUT /tankoubons/{id}`) as well as a plain archive, unlike this
// component's own default `useUpdateArchiveMetadata` mutation.

const MAX_STARS = 5
const DEFAULT_STAR_SIZE = 24
/** Interactive clicks only ever produce whole/half-star values — a directly-edited tag can carry
 * finer precision (`rating:4.3`), which `StarSprite`'s own fractional rendering already displays
 * correctly, but this widget's own pointer granularity deliberately stops at 0.5 (E-Hentai's own
 * rating widget — the direct prior art here — offers the same 10-region-per-5-star granularity,
 * not finer). */
const CLICK_STEP = 0.5

// Clearing a rating is right-click-to-clear on the star row itself rather than a separate trash
// icon — a dedicated icon needed its own reserved layout space (so an unrated row didn't render
// narrower and misalign anything sized off this widget, e.g. the Library context menu) and read
// as visually redundant next to 5 already-interactive stars. Not self-evident as a gesture on its
// own, so it's paired with an explanatory `Tooltip` on the star row (see the `return` below).

export function currentRating(tags: string): number {
  const match = tags.split(",").find((t) => t.trim().toLowerCase().startsWith("rating:"))
  return match ? (parseRating(match.split(":").slice(1).join(":")) ?? 0) : 0
}

export function RatingWidget({
  archiveId,
  tags,
  size = DEFAULT_STAR_SIZE,
  onChange,
}: {
  archiveId: string
  tags: string
  /** Star glyph size in px — the Library context-menu usage renders this noticeably smaller than
   * the Reader overview's own default, to match the rest of that menu's own compact row height. */
  size?: number
  /** Overrides the default `useUpdateArchiveMetadata`-backed persistence (which always PUTs
   * `/archives/{archiveId}/metadata` — wrong for a tankoubon). Receives the same next-tags string
   * this component would otherwise send itself. */
  onChange?: (nextTags: string) => void
}) {
  const { t } = useTranslation()
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const rating = currentRating(tags)
  // E-Hentai's own rating widget (the direct prior art for this component, per its real
  // `rating_show`/`rating_reset` — verified against `ehg_gallery.c.js`) live-previews whatever
  // score the pointer is currently over instead of only ever showing the persisted value: hovering
  // recomputes the sprite to show what clicking *now* would set, reverting to the real saved
  // rating the moment the pointer leaves. `null` means "not hovering any star" (show `rating`).
  const [previewRating, setPreviewRating] = useState<number | null>(null)
  const displayRating = previewRating ?? rating

  function setRating(score: number | null) {
    const withoutRating = tags
      .split(",")
      .filter((t) => !t.trim().toLowerCase().startsWith("rating:"))
      .map((t) => t.trim())
      .filter(Boolean)
    const next = score ? [...withoutRating, `rating:${formatRating(score)}`] : withoutRating
    const nextTags = next.join(",")
    if (onChange) onChange(nextTags)
    else updateMetadata.mutate({ tags: nextTags })
  }

  /** Half-star-precision pointer target — the star index a pointer lands on plus whether it hit
   * the left (half) or right (whole) side of that star's own box, mirroring E-Hentai's real
   * left-half/right-half `<area>` hit-testing (verified against a real screenshot of its own
   * `<map>` markup: 8px-wide regions inside each 16px star, half/whole alternating). Shared by
   * both the hover-preview (`onMouseMove`) and the actual commit (`onClick`) — E-Hentai's own
   * `rating_show`/`rating_set` compute the identical region for both, just one previews and the
   * other persists. */
  function scoreFromPointer(e: MouseEvent<HTMLSpanElement>, starIndex: number): number {
    const rect = e.currentTarget.getBoundingClientRect()
    const isRightHalf = e.clientX - rect.left > rect.width / 2
    return starIndex + (isRightHalf ? 1 : CLICK_STEP)
  }

  return (
    <Tooltip label={t("Click a star to rate. Right-click to clear the rating.")}>
      <span
        style={{ display: "inline-flex" }}
        onMouseLeave={() => setPreviewRating(null)}
        onContextMenu={(e) => {
          e.preventDefault()
          if (rating > 0) setRating(null)
        }}
      >
        {Array.from({ length: MAX_STARS }, (_, i) => i).map((i) => (
          <span
            key={i}
            style={{ cursor: "pointer" }}
            onMouseMove={(e) => setPreviewRating(scoreFromPointer(e, i))}
            onClick={(e) => setRating(scoreFromPointer(e, i))}
          >
            <StarSprite fraction={Math.max(0, Math.min(1, displayRating - i))} size={size} />
          </span>
        ))}
      </span>
    </Tooltip>
  )
}
