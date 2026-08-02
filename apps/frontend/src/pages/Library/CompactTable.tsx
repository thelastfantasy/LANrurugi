import type { MouseEvent } from 'react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useSettings } from '../../api/hooks'
import type { ArchiveMetadata } from '../../api/types'
import { promptDialog } from '../../dialog'
import { buildSearchToken, formatTimestampForDisplay, getTagSearchURL, tagValueForSearch } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { CUSTOM_COLUMN_PREFIX, DEFAULT_CUSTOM_COLUMNS } from '../../storageKeys'
import { BookmarkIcon, TagLine } from './shared'

/** One compact-table custom column's chosen namespace, read/write straight to its own
 * `localStorage` key (`customColumn${index}`) — ports `generateTableHeaders`'s per-header default
 * (`artist`/`series` for columns 1/2, `Header N` beyond that) and `handleColumnNum`'s rename flow
 * (legacy uses an inline pencil-icon-triggered prompt; reproduced here the same way rather than a
 * persistent input, since this is an infrequent per-column configuration action, not a per-row
 * one). */
function useCustomColumnNamespace(index: number): [string, (v: string) => void] {
  const key = `${CUSTOM_COLUMN_PREFIX}${index}`
  const [namespace, setNamespaceState] = useState(
    () => localStorage.getItem(key) ?? DEFAULT_CUSTOM_COLUMNS[index - 1] ?? `Header ${index}`,
  )
  const setNamespace = (v: string) => {
    setNamespaceState(v)
    localStorage.setItem(key, v)
  }
  return [namespace, setNamespace]
}

function CustomColumnHeader({ index }: { index: number }) {
  const { t } = useTranslation()
  const [namespace, setNamespace] = useCustomColumnNamespace(index)
  return (
    <th>
      {namespace.charAt(0).toUpperCase() + namespace.slice(1)}{' '}
      <i
        className="fas fa-pencil-alt edit-header-btn"
        title={t('Edit this column') ?? undefined}
        style={{ cursor: 'pointer' }}
        onClick={() => {
          void (async () => {
            const next = await promptDialog(t('Tag namespace') ?? '', namespace)
            if (next?.trim()) setNamespace(next.trim())
          })()
        }}
      ></i>
    </th>
  )
}

/** Ports `renderColumn` exactly: extracts every value under this column's chosen namespace out of
 * the archive's full `tags` string (regex, not `splitTagsByNamespace`, to match legacy's own
 * substring-match behavior byte-for-byte), formats dates via `convertTimestamp`, capitalizes
 * every other value's words (skipping `source`, since that's a URL), and links each to a search
 * for that exact tag. */
function CustomColumnCell({
  index,
  tags,
  onSearchTag,
}: {
  index: number
  tags: string
  onSearchTag: (namespacedTag: string) => void
}) {
  const [namespace] = useCustomColumnNamespace(index)
  // Server timezone for `date_added`/`timestamp` custom-column display + search URL — same
  // pattern as `TagTable`/`ArchiveOverviewOverlay`'s own `TagsTable`.
  const timezone = useSettings().data?.timezone ?? ''
  const matches = [...tags.matchAll(new RegExp(`${namespace}:([^,]+)`, 'g'))].map((m) => m[1].trim())
  const isDate = namespace === 'date_added' || namespace === 'timestamp'
  return (
    <td style={{ textAlign: 'left' }}>
      {matches.map((raw, i) => {
        const text = isDate ? formatTimestampForDisplay(raw, timezone) : namespace === 'source' ? raw : raw.replace(/\b./g, (c) => c.toUpperCase())
        return (
          <span key={i}>
            <a
              href={getTagSearchURL(namespace, raw, timezone)}
              style={{ cursor: 'pointer' }}
              onClick={(e) => {
                e.preventDefault()
                // `tagValueForSearch`, not the raw stored value — same real bug class as
                // `TagTable.tsx`'s own fix: a `date_added`/`timestamp` value's search semantics
                // are the `yyyy-mm-dd` day-range syntax, not its bare Unix-seconds form (which
                // never matches, `date_added` isn't tag-indexed). This in-app click path bypassed
                // the same conversion the `href` above already applies.
                onSearchTag(buildSearchToken(namespace, tagValueForSearch(namespace, raw, timezone), !isDate))
              }}
            >
              {text}
            </a>
            {i < matches.length - 1 && ', '}
          </span>
        )
      })}
    </td>
  )
}

/** Compact (table) view of the library grid — extracted from `Library()`'s own inline JSX (the
 * `viewMode === 'compact'` branch), not a pre-existing standalone component, so this file's
 * `CompactTable` itself is new relative to the pre-split `Library.tsx`; every prop it takes was
 * already a value in `Library()`'s own scope, just threaded through as-is rather than newly
 * derived. Column order/content mirrors legacy's real compact-table columns exactly
 * (`index_datatables.js`'s `columns` array): Title, then `columns` editable custom namespace
 * columns (`customColumn1..N`, default Artist/Series), then a single full Tags column (all
 * namespaces, unfiltered) — legacy has no dedicated Pages/Date Added columns here at all (those
 * were an invented approximation, not what legacy actually shows). */
export function CompactTable({
  shown,
  columns,
  selectedIds,
  multiSelect,
  onSearchTag,
  onToggleSelected,
  onOpen,
  onContextMenu,
}: {
  shown: ArchiveMetadata[]
  columns: number
  selectedIds: Set<string>
  multiSelect: boolean
  onSearchTag: (namespacedTag: string) => void
  onToggleSelected: (id: string) => void
  onOpen: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
}) {
  const { t } = useTranslation()
  return (
    <table className="itg" style={{ width: '100%' }}>
      <thead>
        <tr>
          <th>{t('Title')}</th>
          {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
            <CustomColumnHeader key={i} index={i} />
          ))}
          <th>{t('Tags')}</th>
        </tr>
      </thead>
      <tbody>
        {shown.map((a) => (
          <tr
            key={a.arcid}
            className={selectedIds.has(a.arcid) ? 'msm-selected' : undefined}
            onContextMenu={(e) => onContextMenu(e, a)}
          >
            <td style={{ textAlign: 'left' }}>
              {multiSelect && (
                <input
                  type="checkbox"
                  checked={selectedIds.has(a.arcid)}
                  onChange={() => onToggleSelected(a.arcid)}
                  style={{ marginRight: 6 }}
                />
              )}
              <BookmarkIcon archiveId={a.arcid} />{' '}
              <a
                href={routes.reader(a.arcid)}
                onClick={(e) => {
                  e.preventDefault()
                  if (multiSelect) onToggleSelected(a.arcid)
                  else onOpen(a.arcid)
                }}
              >
                {a.isnew && '🆕 '}
                {a.title}
              </a>
            </td>
            {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
              <CustomColumnCell key={i} index={i} tags={a.tags} onSearchTag={onSearchTag} />
            ))}
            <td style={{ textAlign: 'left' }}>
              <TagLine tags={a.tags} onSearchTag={onSearchTag} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
