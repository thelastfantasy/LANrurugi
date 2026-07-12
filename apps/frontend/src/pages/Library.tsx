import type { MouseEvent } from 'react'
import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import {
  useArchives,
  useCategories,
  useCreateTankoubon,
  useSearch,
  useServerInfo,
  useTankoubons,
} from '../api/hooks'
import type { ArchiveMetadata } from '../api/types'
import { toast } from '../toast'
import { useDocumentTitle } from '../useDocumentTitle'
import { recordSearchNavigation } from './Reader/crossArchiveNav'

// Matches `lanrurugi-api::search`'s own fixed page size (`search.rs`'s `PAGE_SIZE` constant) —
// server-side pagination isn't configurable per-request, so "Go to Page" paginates through
// exactly these fixed 100-archive chunks rather than the user's own `archives_per_page` display
// setting (a real, minor mismatch versus legacy, which the same setting also doesn't control this
// exact cursor for).
const PAGE_SIZE = 100

interface ContextMenuState {
  archive: ArchiveMetadata
  x: number
  y: number
}

// Mirrors legacy's exact thumbnail card markup (`buildThumbnailDiv` in
// `~/LANraragi/public/js/mod/common.js`) — `div.id1` > (`div.id2` title, `div.id3` cover image,
// `div.id4` page count + tags) — so the copied theme CSS (`useApplyTheme`) styles it identically.
// Doesn't reproduce the tag-hover tooltip (`buildTagTooltip`, Tippy.js) — a separate, unbuilt,
// purely cosmetic feature. Right-click opens `ArchiveContextMenu` below (real functional parity,
// not the tooltip); multi-select mode overlays a checkbox instead of navigating on click.
function ArchiveCard({
  archive,
  multiSelect,
  selected,
  onToggleSelect,
  onContextMenu,
  onOpen,
}: {
  archive: ArchiveMetadata
  multiSelect: boolean
  selected: boolean
  onToggleSelect: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const id = archive.arcid

  function handleOpen(e: MouseEvent) {
    e.preventDefault()
    if (multiSelect) {
      onToggleSelect(id)
    } else {
      onOpen(id)
    }
  }

  return (
    <div className={`id1${selected ? ' msm-selected' : ''}`} id={id} onContextMenu={(e) => onContextMenu(e, archive)}>
      <div className="id2">
        {archive.isnew && <span title={t('New!') ?? undefined}>🆕</span>}
        <a href={`#/reader/${id}`} title={archive.title} onClick={handleOpen}>
          {archive.title}
        </a>
        {!multiSelect && (
          <a
            href={`#/edit/${id}`}
            title={t('Edit') ?? undefined}
            style={{ marginLeft: 6 }}
            onClick={(e) => {
              e.preventDefault()
              e.stopPropagation()
              navigate(`/edit/${id}`)
            }}
          >
            <i className="fa fa-pen"></i>
          </a>
        )}
      </div>
      <div className="id3" style={{ position: 'relative' }}>
        {multiSelect && (
          <input
            type="checkbox"
            checked={selected}
            onChange={() => onToggleSelect(id)}
            style={{ position: 'absolute', top: 6, left: 6, zIndex: 1, width: 20, height: 20 }}
          />
        )}
        <a href={`#/reader/${id}`} title={archive.title} onClick={handleOpen}>
          <img src={`/api/archives/${id}/thumbnail`} alt={archive.title} loading="lazy" />
        </a>
      </div>
      <div className="id4">
        <span>
          {archive.progress ?? 0} / {archive.pagecount} {t('pages')}
        </span>
        {archive.tags && <span className="tags">{archive.tags}</span>}
      </div>
    </div>
  )
}

// Mirrors legacy's own right-click menu (`~/LANraragi/public/js/mod/index_contextmenu.js`,
// `jquery.contextMenu`) — same action set (open, edit, download, add to category, delete), a
// plain absolutely-positioned React div instead of a jQuery plugin (constitution/user direction:
// no jQuery-family dependencies in this rewrite). Closes on any outside click or `Escape`.
function ArchiveContextMenu({
  state,
  categories,
  onClose,
  onAddToCategory,
  onDelete,
  onOpen,
}: {
  state: ContextMenuState
  categories: { id: string; name: string; search: string | null }[] | undefined
  onClose: () => void
  onAddToCategory: (categoryId: string, archiveId: string) => void
  onDelete: (archiveId: string) => void
  onOpen: (id: string) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { archive, x, y } = state
  const staticCategories = (categories ?? []).filter((c) => !c.search)

  return (
    <>
      {/* Full-viewport transparent overlay — the standard "click outside to dismiss" pattern for
          a positioned popup, cheaper than a document-level listener + ref check. */}
      <div style={{ position: 'fixed', inset: 0, zIndex: 1000 }} onClick={onClose} onContextMenu={(e) => { e.preventDefault(); onClose() }} />
      <div
        className="id1"
        style={{
          position: 'fixed',
          top: y,
          left: x,
          zIndex: 1001,
          width: 220,
          padding: '6px 0',
          textAlign: 'left',
        }}
      >
        <ul style={{ listStyle: 'none', margin: 0, padding: 0 }}>
          <li className="context-menu-item" style={{ padding: '4px 12px', cursor: 'pointer' }} onClick={() => { onClose(); onOpen(archive.arcid) }}>
            <i className="fa fa-book-open" style={{ width: 18 }}></i> {t('Read')}
          </li>
          <li className="context-menu-item" style={{ padding: '4px 12px', cursor: 'pointer' }} onClick={() => { onClose(); navigate(`/edit/${archive.arcid}`) }}>
            <i className="fa fa-pen" style={{ width: 18 }}></i> {t('Edit')}
          </li>
          <li className="context-menu-item" style={{ padding: '4px 12px', cursor: 'pointer' }} onClick={() => { onClose(); window.location.assign(`/api/archives/${archive.arcid}/download`) }}>
            <i className="fa fa-download" style={{ width: 18 }}></i> {t('Download')}
          </li>
          {staticCategories.length > 0 && (
            <li style={{ padding: '4px 12px', borderTop: '1px solid rgba(128,128,128,0.3)', marginTop: 4 }}>
              <div style={{ opacity: 0.7, fontSize: '0.85em' }}>{t('Add to Category :')}</div>
              <ul style={{ listStyle: 'none', margin: 0, padding: 0, maxHeight: 160, overflowY: 'auto' }}>
                {staticCategories.map((c) => (
                  <li
                    key={c.id}
                    className="context-menu-item"
                    style={{ padding: '2px 4px', cursor: 'pointer' }}
                    onClick={() => { onClose(); onAddToCategory(c.id, archive.arcid) }}
                  >
                    {c.name}
                  </li>
                ))}
              </ul>
            </li>
          )}
          <li
            className="context-menu-item"
            style={{ padding: '4px 12px', cursor: 'pointer', borderTop: '1px solid rgba(128,128,128,0.3)', marginTop: 4 }}
            onClick={() => {
              onClose()
              if (window.confirm(t('This will delete both metadata and matching files from your system! Please use with caution.') ?? undefined)) {
                onDelete(archive.arcid)
              }
            }}
          >
            <i className="fa fa-trash" style={{ width: 18 }}></i> {t('Delete Archive')}
          </li>
        </ul>
      </div>
    </>
  )
}

export default function Library() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useDocumentTitle()
  const info = useServerInfo()
  const archives = useArchives()
  const categories = useCategories()
  const tankoubons = useTankoubons()
  const createTankoubon = useCreateTankoubon()

  const [filterInput, setFilterInput] = useState('')
  const [appliedFilter, setAppliedFilter] = useState('')
  const [selectedCategory, setSelectedCategory] = useState('')
  const [sortby, setSortby] = useState<'title' | 'date_added'>('title')
  const [order, setOrder] = useState<'asc' | 'desc'>('asc')
  const [columns, setColumns] = useState(6)
  const [page, setPage] = useState(0)
  const [multiSelect, setMultiSelect] = useState(false)
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set())
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null)

  // A plain, unfiltered `useArchives()` isn't enough on its own to answer "how many total
  // archives are there" once a category/sort/page is active — `/search` (empty filter included)
  // is the single source of truth for everything shown here, matching legacy's own `index.js`,
  // which always goes through the same DataTables-backed search endpoint regardless of whether a
  // text filter is actually set.
  const search = useSearch({
    filter: appliedFilter,
    category: selectedCategory || undefined,
    sortby,
    order,
    start: page * PAGE_SIZE,
  })

  const shown = search.data?.data ?? []
  const totalFiltered = search.data?.recordsFiltered ?? 0
  const pageCount = Math.max(1, Math.ceil(totalFiltered / PAGE_SIZE))

  async function handleCreateTankoubon() {
    const name = window.prompt(t('Title:') ?? 'Title:')
    if (!name?.trim()) return
    const result = await createTankoubon.mutateAsync(name.trim())
    navigate(`/tankoubon/${result.tankid}/edit`)
  }

  function toggleCategory(id: string) {
    setSelectedCategory((prev) => (prev === id ? '' : id))
    setPage(0)
  }

  function toggleSelected(id: string) {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  function selectAllOnPage() {
    setSelectedIds((prev) => {
      const next = new Set(prev)
      for (const a of shown) next.add(a.arcid)
      return next
    })
  }

  function clearSelection() {
    setSelectedIds(new Set())
  }

  function runBatchOnSelection() {
    if (selectedIds.size === 0) return
    // Matches legacy's own hand-off exactly (`~/LANraragi/public/js/mod/index.js`'s
    // `openBatchOnSelection`/`updateSelectionCount`): stash the selection in `localStorage` under
    // the same key, open `/batch` in a new tab to read (and immediately clear) it.
    localStorage.setItem('msmSelection', JSON.stringify([...selectedIds]))
    window.open('/batch', '_blank')
  }

  async function mergeSelectionIntoTankoubon() {
    if (selectedIds.size === 0) return
    const name = window.prompt(t('Enter a name for the new Tankoubon.') ?? undefined)
    if (!name?.trim()) return
    try {
      const result = await createTankoubon.mutateAsync(name.trim())
      await fetch(`/api/tankoubons/${result.tankid}`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ archives: [...selectedIds] }),
      })
      clearSelection()
      navigate(`/tankoubon/${result.tankid}/edit`)
    } catch {
      toast({ heading: t('Error creating Tankoubon') ?? undefined, icon: 'error' })
    }
  }

  async function addArchiveToCategory(categoryId: string, archiveId: string) {
    await fetch(`/api/categories/${categoryId}/${archiveId}`, { method: 'PUT' })
  }

  async function deleteArchive(archiveId: string) {
    await fetch(`/api/archives/${archiveId}`, { method: 'DELETE' })
    await search.refetch()
  }

  function handleContextMenu(e: MouseEvent, archive: ArchiveMetadata) {
    e.preventDefault()
    setContextMenu({ archive, x: e.clientX, y: e.clientY })
  }

  // Hands off "which search produced this results page" to the reader (`crossArchiveNav.ts`),
  // matching legacy's own datatables->reader handoff — lets `,`/`.` step across archives inside
  // this same search without the reader re-deriving it.
  function handleOpenArchive(id: string) {
    recordSearchNavigation(
      shown.map((a) => a.arcid),
      page + 1,
      {
        filter: appliedFilter,
        category: selectedCategory,
        sortby,
        order,
        pageSize: PAGE_SIZE,
        groupbyTanks: true,
        hidecompleted: false,
      },
    )
    navigate(`/reader/${id}`)
  }

  if (search.isError) {
    return (
      <p className="p-6 text-red-500">
        {t('Failed to load archives: {{error}}', { error: String(search.error) })}
      </p>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{info.data?.motd}</h1>

      <div id="toppane">
        <div className="idi">
          <div id="category-container">
            {categories.data?.map((c) => (
              <button
                key={c.id}
                type="button"
                className="favtag-btn"
                style={selectedCategory === c.id ? { fontWeight: 'bold', textDecoration: 'underline' } : undefined}
                onClick={() => toggleCategory(c.id)}
              >
                {c.name}
              </button>
            ))}
          </div>
          <input
            id="search-input"
            className="search stdinput"
            value={filterInput}
            onChange={(e) => setFilterInput(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') {
                setAppliedFilter(filterInput)
                setPage(0)
              }
            }}
            placeholder={t('Search Title, Artist, Series, Language or Tags') ?? undefined}
          />
          <input
            id="apply-search"
            className="searchbtn stdbtn"
            type="button"
            value={t('Apply Filter') ?? undefined}
            onClick={() => {
              setAppliedFilter(filterInput)
              setPage(0)
            }}
          />
          <input
            id="clear-search"
            className="searchbtn stdbtn"
            type="button"
            value={t('Clear Filter') ?? undefined}
            onClick={() => {
              setFilterInput('')
              setAppliedFilter('')
              setSelectedCategory('')
              setPage(0)
            }}
          />
          <input
            id="msm-toggle"
            className="searchbtn stdbtn"
            type="button"
            value={t('Select Archives') ?? undefined}
            onClick={() => {
              setMultiSelect((v) => !v)
              clearSelection()
            }}
          />
        </div>
      </div>

      {multiSelect && (
        <div
          id="msm-carousel-controls"
          style={{ display: 'flex', alignItems: 'center', gap: 12, justifyContent: 'center', margin: '8px 0' }}
        >
          <span id="msm-selection-count">{t('${n} selected').replace('${n}', String(selectedIds.size))}</span>
          <a href="#" title={t('Run Batch Operations on selection') ?? undefined} onClick={(e) => { e.preventDefault(); runBatchOnSelection() }}>
            <i className="fa fa-2x fa-hammer"></i>
          </a>
          <a href="#" title={t('Merge Archives into Tankoubon') ?? undefined} onClick={(e) => { e.preventDefault(); void mergeSelectionIntoTankoubon() }}>
            <i className="fa fa-2x fa-compress-alt"></i>
          </a>
          <a href="#" title={t('Clear selection') ?? undefined} onClick={(e) => { e.preventDefault(); clearSelection() }}>
            <i className="fa fa-2x fa-eject"></i>
          </a>
          <a href="#" title={t('Select All in Page') ?? undefined} onClick={(e) => { e.preventDefault(); selectAllOnPage() }}>
            <i className="fa fa-2x fa-check-double"></i>
          </a>
        </div>
      )}

      <div className="option-flyout" style={{ textAlign: 'left', margin: '10px' }}>
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
          <span style={{ fontWeight: 'bold' }}>{t('Groupings')}</span>
          <a href="#" onClick={(e) => { e.preventDefault(); void handleCreateTankoubon() }}>
            {t('Add')}
          </a>
        </div>
        {tankoubons.data && tankoubons.data.result.length > 0 && (
          <ul style={{ textAlign: 'left', marginTop: 8 }}>
            {tankoubons.data.result.map((tankoubon) => (
              <li key={tankoubon.id} style={{ padding: '4px 0' }}>
                <a
                  href={`#/tankoubon/${tankoubon.id}/edit`}
                  onClick={(e) => {
                    e.preventDefault()
                    navigate(`/tankoubon/${tankoubon.id}/edit`)
                  }}
                >
                  {tankoubon.name}
                </a>{' '}
                ({t('{{count}} volumes, in order', { count: tankoubon.archives.length })})
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="table-options" style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 16, margin: '0 10px' }}>
        <div className="thumbnail-options">
          {t('Sort by:')}{' '}
          <select
            className="favtag-btn"
            value={sortby}
            onChange={(e) => {
              setSortby(e.target.value as 'title' | 'date_added')
              setPage(0)
            }}
          >
            <option value="title">{t('Title')}</option>
            <option value="date_added">{t('Date')}</option>
          </select>
          <a
            className="fa fa-sort-alpha-down table-option"
            href="#"
            title={t('Sort Order') ?? undefined}
            style={{ marginLeft: 6 }}
            onClick={(e) => {
              e.preventDefault()
              setOrder((o) => (o === 'asc' ? 'desc' : 'asc'))
            }}
          >
            {order === 'asc' ? '↑' : '↓'}
          </a>
        </div>
        <div className="compact-options">
          {t('Columns:')}{' '}
          {/* Legacy's own Columns dropdown (`index.html.tt2`) is 1-20 with no "auto" concept —
              matched exactly here rather than adding one, so `columns` always starts at a real
              grid width instead of the flex-wrap fallback this component also supports. */}
          <select className="favtag-btn" value={columns} onChange={(e) => setColumns(Number(e.target.value))}>
            {Array.from({ length: 20 }, (_, i) => i + 1).map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
        </div>
        <div style={{ marginLeft: 'auto' }}>
          {t('Go to Page:')}{' '}
          <select className="favtag-btn" value={page} onChange={(e) => setPage(Number(e.target.value))}>
            {Array.from({ length: pageCount }, (_, i) => i).map((p) => (
              <option key={p} value={p}>
                {p + 1}
              </option>
            ))}
          </select>
        </div>
      </div>

      <h2 style={{ fontWeight: 'bold', margin: '10px' }}>
        {appliedFilter.trim() || selectedCategory
          ? t('{{count}} results', { count: totalFiltered })
          : t('Archives ({{count}})', { count: archives.data?.length ?? totalFiltered })}
      </h2>
      {search.isLoading ? (
        <p>{t('Loading library…')}</p>
      ) : (
        <div
          id="thumbs_container"
          style={
            columns > 0
              ? { display: 'grid', gridTemplateColumns: `repeat(${columns}, 1fr)`, gap: 8, justifyItems: 'center' }
              : { display: 'flex', flexWrap: 'wrap', justifyContent: 'center' }
          }
        >
          {shown.map((a) => (
            <ArchiveCard
              key={a.arcid}
              archive={a}
              multiSelect={multiSelect}
              selected={selectedIds.has(a.arcid)}
              onToggleSelect={toggleSelected}
              onContextMenu={handleContextMenu}
              onOpen={handleOpenArchive}
            />
          ))}
        </div>
      )}

      {contextMenu && (
        <ArchiveContextMenu
          state={contextMenu}
          categories={categories.data}
          onClose={() => setContextMenu(null)}
          onAddToCategory={(categoryId, archiveId) => void addArchiveToCategory(categoryId, archiveId)}
          onDelete={(archiveId) => void deleteArchive(archiveId)}
          onOpen={handleOpenArchive}
        />
      )}
    </div>
  )
}
