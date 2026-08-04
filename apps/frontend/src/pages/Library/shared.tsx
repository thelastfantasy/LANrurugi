import type { MouseEvent } from 'react'
import { useTranslation } from 'react-i18next'

import { useBookmarkLink, useCategories, useLoginStatus, useSettings } from '../../api/hooks'
import type { ArchiveMetadata } from '../../api/types'
import { TagTable } from '../../components/TagTable'
import { Tooltip } from '../../components/Tooltip'
import { buildSearchToken, colorCodeTags, TIMESTAMP_NAMESPACE } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { toast } from '../../toast'

// Matches `lanrurugi-api::search`'s fixed page size (`search.rs`'s `PAGE_SIZE` constant) —
// server-side pagination isn't configurable per-request, so "Go to Page" paginates through these
// fixed 100-archive chunks rather than the user's own `archives_per_page` display setting.
export const PAGE_SIZE = 100

// The two hardcoded quick-filter category ids legacy's own `index.js` special-cases
// (`LANraragi::Controller::Api::Search::handle_databases`) — not real category ids, intercepted
// client-side before ever reaching `category=` and turned into `newonly=true`/`untaggedonly=true`
// instead.
export const NEW_ONLY = 'NEW_ONLY'

export const UNTAGGED_ONLY = 'UNTAGGED_ONLY'

// Legacy caps the visible category-button row at 10 entries before spilling the rest into a
// "..." overflow `<select>` (`index.js`'s `loadCategories`).
export const CATEGORY_BUTTON_CAP = 10

export type CarouselMode = 'ondeck' | 'random' | 'inbox' | 'untagged'

export interface ContextMenuState {
  archive: ArchiveMetadata
  x: number
  y: number
  source: 'grid' | 'carousel'
}

export const CAROUSEL_ICON: Record<CarouselMode, string> = {
  ondeck: 'fa-book-reader',
  random: 'fa-random',
  inbox: 'fa-envelope-open-text',
  untagged: 'fa-edit',
}

export function isTankoubonId(id: string): boolean {
  return id.startsWith('TANK_')
}

/** Bookmark star — ports `buildBookmarkIconElement`: renders nothing unless a bookmark category
 * is actually configured (`useBookmarkLink`), filled/outline depending on current membership
 * (read straight off `useCategories`' own `archives` array, matching the Reader page's own
 * `isBookmarked` derivation — no separate `localStorage.bookmarkedArchives` cache, since that
 * cache exists in legacy purely to avoid a page-load fetch we don't need with react-query's own
 * shared cache), and disabled/dimmed when logged out. */
export function BookmarkIcon({ archiveId }: { archiveId: string }) {
  const { t } = useTranslation()
  const bookmarkLink = useBookmarkLink()
  const categories = useCategories()
  const loginStatus = useLoginStatus()
  const bookmarkCategoryId = bookmarkLink.data?.category_id || null
  if (!bookmarkCategoryId) return null
  const loggedIn = loginStatus.data?.logged_in ?? true
  const isBookmarked = Boolean(
    categories.data?.find((c) => c.id === bookmarkCategoryId)?.archives.includes(archiveId),
  )

  async function toggle(e: MouseEvent) {
    e.preventDefault()
    e.stopPropagation()
    if (!loggedIn) {
      toast({
        text: `<a href="${routes.login()}">${t('Login')}</a> ${t('to toggle bookmark feature.')}`,
        icon: 'warning',
      })
      return
    }
    const method = isBookmarked ? 'DELETE' : 'PUT'
    await fetch(`/api/categories/${bookmarkCategoryId}/${archiveId}`, { method })
    await categories.refetch()
  }

  return (
    <i
      className={`${isBookmarked ? 'fas' : 'far'} fa-bookmark thumbnail-bookmark-icon${loggedIn ? '' : ' disabled'}`}
      title={t('Toggle Bookmark') ?? undefined}
      style={!loggedIn ? { opacity: 0.5, cursor: 'not-allowed' } : { cursor: 'pointer' }}
      onClick={(e) => void toggle(e)}
    ></i>
  )
}

/** Tag line + hover tooltip — ports `colorCodeTags` (namespace-colored, date/time-excluded,
 * CSS-ellipsis-truncated via the `span.tags` rule already present in the copied `lrr.css`) for the
 * always-visible line, and `buildTagsDiv` (the full per-namespace tag table, via the shared
 * `TagTable` component) for the hover body — rendered through the shared `Tooltip` component
 * (portaled to `document.body`) rather than a locally absolutely-positioned `<table>`, since the
 * grid card's own ancestors clip an unportaled tooltip (this was silently never visible on the
 * homepage grid before — a real regression fixed here, not a style tweak). Click-to-search on any
 * individual tag (`.gt[search]` in legacy, intercepted by `index_datatables.js` to fire a live
 * search instead of a full navigation — reproduced here as an in-app filter-apply). */
export function TagLine({
  tags,
  onSearchTag,
}: {
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const timezone = useSettings().data?.timezone ?? ''
  const coded = colorCodeTags(tags, timezone)
  if (coded.length === 0) return null

  return (
    <Tooltip
      label={<TagTable tags={tags} onSearchTag={(ns, v) => onSearchTag(buildSearchToken(ns, v, !TIMESTAMP_NAMESPACE.test(ns)))} />}
      wrapperStyle={{ display: 'block' }}
    >
      <span className="tags tag-tooltip">
        {coded.map((tag, i) => (
          <span key={i}>
            <span
              className={`${tag.namespace}-tag`}
              style={{ cursor: 'pointer' }}
              onClick={(e) => {
                e.preventDefault()
                e.stopPropagation()
                onSearchTag(tag.text)
              }}
            >
              {tag.text}
            </span>
            {i < coded.length - 1 && ', '}
          </span>
        ))}
      </span>
    </Tooltip>
  )
}
