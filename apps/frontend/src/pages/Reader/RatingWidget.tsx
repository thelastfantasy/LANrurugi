import { useTranslation } from 'react-i18next'

import { useUpdateArchiveMetadata } from '../../api/hooks'

// Mirrors legacy's Raty-based rating widget (`~/LANraragi/public/js/reader.js:315-337`) — there is
// no dedicated rating field or column anywhere in legacy's own data model; a rating is just a tag
// under the `rating:` namespace whose value is N repetitions of the star emoji (`rating:⭐⭐⭐` for
// 3 stars), so setting/clearing it is a plain tags-string rewrite via the same
// `PUT /archives/{id}/metadata?tags=` endpoint every other tag edit uses.

const MAX_STARS = 5
const STAR = '⭐'

function currentRating(tags: string): number {
  const match = tags.split(',').find((t) => t.trim().toLowerCase().startsWith('rating:'))
  if (!match) return 0
  const value = match.split(':').slice(1).join(':').trim()
  return [...value].filter((c) => c === STAR).length
}

export default function RatingWidget({ archiveId, tags }: { archiveId: string; tags: string }) {
  const { t } = useTranslation()
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const rating = currentRating(tags)

  function setRating(score: number | null) {
    const withoutRating = tags
      .split(',')
      .filter((t) => !t.trim().toLowerCase().startsWith('rating:'))
      .map((t) => t.trim())
      .filter(Boolean)
    const next = score
      ? [...withoutRating, `rating:${STAR.repeat(score)}`]
      : withoutRating
    updateMetadata.mutate({ tags: next.join(',') })
  }

  return (
    <div>
      {Array.from({ length: MAX_STARS }, (_, i) => i + 1).map((n) => (
        <i
          key={n}
          className={n <= rating ? 'fas fa-star' : 'far fa-star'}
          style={{ fontSize: '1.5em', cursor: 'pointer', marginRight: 4 }}
          onClick={() => setRating(n)}
        />
      ))}
      {rating > 0 && (
        <i
          className="fas fa-trash raty-cancel"
          style={{ cursor: 'pointer' }}
          title={t('Clear Rating') ?? undefined}
          onClick={() => setRating(null)}
        />
      )}
    </div>
  )
}
