import type { MouseEvent } from 'react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useSettings } from '../../api/hooks'
import type { ArchiveMetadata } from '../../api/types'
import Tooltip from '../../components/Tooltip'
import { promptDialog } from '../../dialog'
import { buildSearchToken, formatTimestampForDisplay, getTagSearchURL, tagValueForSearch } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { CUSTOM_COLUMN_PREFIX, DEFAULT_CUSTOM_COLUMNS } from '../../storageKeys'
import { BookmarkIcon, isTankoubonId, TagLine } from './shared'

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

/** The `<a>` inside each sortable `<th>` — real legacy markup: `<th id="titleheader"><a>Title</a>
 * </th>`/`<th id="customheader1"><i class="edit-header-btn">…</i><a id="header-1">Artist</a></th>`
 * (`generateTableHeaders`, `index.js`). DataTables auto-binds a click handler to every `<a>` in a
 * `thead th` and toggles `sorting_asc`/`sorting_desc` on the parent `<th>` — the caret itself is a
 * pure CSS `::after` on the `<a>` (`table thead th a:after`, `lrr.css`), driven purely by that
 * class on the ancestor `<th>` (see `<th className>` at each caller), not a glyph rendered here —
 * a real, live-confirmed gap this port was missing entirely: the compact table's headers rendered
 * as plain static text (or, for custom columns, only the unrelated rename-pencil icon), with no
 * way to sort by clicking them at all, unlike every other real legacy instance. `orderable`
 * (`false` only for the Tags column, matching `index_datatables.js`'s own `columns` array) skips
 * the `<a>` entirely, rendering plain text instead — clicking Tags does nothing in legacy either. */
function SortableHeaderLink({
  label,
  sortKey,
  onSort,
}: {
  label: string
  sortKey: string
  onSort: (key: string) => void
}) {
  return (
    <a
      href="#"
      onClick={(e) => {
        e.preventDefault()
        onSort(sortKey)
      }}
    >
      {label}
    </a>
  )
}

function CustomColumnHeader({
  index,
  sortby,
  order,
  onSort,
}: {
  index: number
  sortby: string
  order: 'asc' | 'desc'
  onSort: (key: string) => void
}) {
  const { t } = useTranslation()
  const [namespace, setNamespace] = useCustomColumnNamespace(index)
  const label = namespace.charAt(0).toUpperCase() + namespace.slice(1)
  return (
    // `id="customheaderN"` — matches real legacy's own `generateTableHeaders` markup
    // (`<th id="customheader${i}">`, `index.js`) and is what `lrr.css`'s own
    // `[id^="customheader"] { max-width: 100px }` rule targets, but `max-width` alone doesn't
    // actually participate in `table-layout: fixed`'s own column-width calculation per spec — only
    // an explicit `width` on a first-row cell does (confirmed live: even real legacy's own column
    // isn't clamped to a literal 100px, it just ends up *narrower* than an unconstrained column,
    // because legacy's own DataTables instance additionally sets a real inline `width` on every
    // header `<th>` via its own internal auto-sizing pass — an undocumented, non-trivial algorithm
    // this port doesn't replicate). `width={100}` here is this port's own real (not legacy-copied)
    // fix for the same underlying problem — a real, live-confirmed bug otherwise: with no `width`
    // hint on any header cell at all, the browser's own `table-layout: fixed` default (divide the
    // table's width equally across every column, ignoring content) gave this sparsely-populated
    // custom column the exact same share as Title/Tags, visibly ballooning it far wider than its
    // own content ever needs while starving the two columns that actually have long text to show.
    // Verified live: this single `width` hint alone is enough to make the browser's own fixed-
    // layout algorithm redistribute the freed space to Title/Tags, without needing to also hint
    // *their* widths explicitly.
    <th id={`customheader${index}`} style={{ width: 100 }} className={sortby === namespace ? `sorting_${order}` : undefined}>
      <i
        className="fas fa-pencil-alt edit-header-btn"
        title={t('Edit this column') ?? undefined}
        style={{ cursor: 'pointer' }}
        onClick={(e) => {
          // Distinct from the header's own sort-by-click `<a>` right next to it (real legacy
          // markup renders both siblings in the same `<th>`, `generateTableHeaders`) —
          // `stopPropagation` isn't actually needed here (this `<i>` isn't nested inside the
          // `<a>`), but the click must not also fall through to anything else in this `<th>`.
          e.stopPropagation()
          void (async () => {
            const next = await promptDialog(t('Tag namespace') ?? '', namespace)
            if (next?.trim()) setNamespace(next.trim())
          })()
        }}
      ></i>{' '}
      <SortableHeaderLink label={label} sortKey={namespace} onSort={onSort} />
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
  // `customheader${index}` MUST come before `itd` in the class string — a real, live-confirmed
  // bug otherwise: `lrr.css`'s own `td[class^="customheader"] { max-width: 100px }` rule is a
  // literal *attribute-value* prefix match against the whole `class` string, not "any class in
  // the space-separated list starts with this" — `class="itd customheaderN"` (itd first) never
  // matches `[class^="customheader"]` at all, since the attribute's own string value starts with
  // "itd ", not "customheader". Legacy's own real markup (`index_datatables.js`, `className:`)
  // puts `customheaderN` first for exactly this reason — matched here to actually pick up the
  // real 100px cap instead of silently falling back to whatever width the column's own content
  // happens to want.
  return (
    <td className={`customheader${index} itd`} style={{ textAlign: 'left' }}>
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
  sortby,
  order,
  onSort,
  onSearchTag,
  onToggleSelected,
  onOpen,
  onContextMenu,
}: {
  shown: ArchiveMetadata[]
  columns: number
  selectedIds: Set<string>
  multiSelect: boolean
  /** Current sort namespace/direction, threaded straight from `Library()`'s own `sortby`/`order`
   * URL-driven state (the same state the "Sort by:" dropdown above this table already reads/
   * writes) — clicking a header sorts the exact same way, matching legacy's real behavior of both
   * controls sharing one underlying DataTables sort order. */
  sortby: string
  order: 'asc' | 'desc'
  onSort: (key: string) => void
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
          {/* `id="titleheader"`/`id="tagsheader"` (matching real legacy's own `generateTableHeaders`
              markup, `index.js`) are what `lrr.css`'s own `#titleheader{max-width:400px}`/
              `#tagsheader{max-width:250px}` rules actually target — a real, live-confirmed bug
              otherwise: under `table-layout: fixed`, the rendered column width is locked by the
              header row's own cell width, not by each body `<td>`'s own `max-width` (see
              `CustomColumnHeader`'s own matching `id="customheaderN"` fix for the full reasoning).
              Without these ids, Title/Tags had no width constraint from the header row at all. */}
          <th id="titleheader" className={sortby === 'title' ? `sorting_${order}` : undefined}>
            <SortableHeaderLink label={t('Title')} sortKey="title" onSort={onSort} />
          </th>
          {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
            <CustomColumnHeader key={i} index={i} sortby={sortby} order={order} onSort={onSort} />
          ))}
          {/* Tags column is `orderable: false` in legacy (`index_datatables.js`'s own `columns`
              array) — plain text, no `<a>`/click handler, matching that real restriction. */}
          <th id="tagsheader">{t('Tags')}</th>
        </tr>
      </thead>
      <tbody>
        {shown.map((a) => (
          <tr
            key={a.arcid}
            className={selectedIds.has(a.arcid) ? 'msm-selected' : undefined}
            onContextMenu={(e) => onContextMenu(e, a)}
          >
            <td className="itd title" style={{ textAlign: 'left' }}>
              {multiSelect && (
                <input
                  type="checkbox"
                  checked={selectedIds.has(a.arcid)}
                  onChange={() => onToggleSelected(a.arcid)}
                  style={{ marginRight: 6 }}
                />
              )}
              <BookmarkIcon archiveId={a.arcid} />{' '}
              {/* Legacy's own compact-table title link (`renderTitle`, `index_datatables.js`)
                  shows a 300px-tall cover-thumbnail preview on hover (`buildImageTooltip`) — a
                  real, live-reported gap in this port (row confirmed missing it entirely, no
                  hover preview at all). `wrapperStyle={{ display: 'inline' }}` — `Tooltip`'s own
                  default `inline-flex` wrapper would otherwise interrupt this cell's normal
                  inline text flow (the bookmark icon/checkbox before it, the ellipsis truncation
                  from `.itd` after it), same reasoning as `PageGridCell.tsx`'s own
                  `wrapperStyle` override. */}
              <Tooltip
                label={
                  <img
                    src={
                      isTankoubonId(a.arcid)
                        ? `/api/tankoubons/${a.arcid}/thumbnail?no_fallback=true`
                        : `/api/archives/${a.arcid}/thumbnail?no_fallback=true`
                    }
                    alt=""
                    style={{ height: 300, display: 'block' }}
                  />
                }
                wrapperStyle={{ display: 'inline' }}
              >
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
              </Tooltip>
            </td>
            {Array.from({ length: columns }, (_, i) => i + 1).map((i) => (
              <CustomColumnCell key={i} index={i} tags={a.tags} onSearchTag={onSearchTag} />
            ))}
            <td className="itd tags" style={{ textAlign: 'left' }}>
              <TagLine tags={a.tags} onSearchTag={onSearchTag} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  )
}
