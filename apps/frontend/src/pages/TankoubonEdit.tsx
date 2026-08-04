import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate, useParams } from 'react-router-dom'

import {
  useAddToTankoubon,
  useArchiveMetadata,
  useDeleteTankoubon,
  useSearch,
  useStats,
  useTankoubon,
  useUpdateTankoubon,
} from '../api/hooks'
import type { TankoubonMetadata } from '../api/types'
import { PopupMenu, PopupMenuItem } from '../components/PopupMenu'
import SortableList from '../components/SortableList'
import TagInput from '../components/TagInput'
import Tooltip from '../components/Tooltip'
import { routes } from '../routes'
import { toast } from '../toast'
import { useDocumentTitle } from '../useDocumentTitle'

/** Resolves an archive ID to its real title for the archive-list row below, with a
 * hover-thumbnail tooltip — matching real legacy's own `edit.html.tt2` (`is_tank` branch, line
 * 147: `archive.title` with `onmouseover="IndexTable.buildImageTooltip(this)"`), not a bare ID.
 * A standalone component (not a hook called in a loop) since the archive list is
 * variable-length, same reasoning as `RecentlyAddedCarousel.tsx`'s own
 * `SelectedArchiveSlideContent`. Falls back to the raw ID while its own fetch is in flight or if
 * it fails, so a row is never blank. */
function ArchiveTitle({ archiveId }: { archiveId: string }) {
  const metadata = useArchiveMetadata(archiveId)
  if (!metadata.data) return <span>{archiveId}</span>
  return (
    <Tooltip
      anchor="cursor"
      wrapperStyle={{ display: 'inline' }}
      label={
        <img
          src={`/api/archives/${archiveId}/thumbnail?no_fallback=true`}
          alt=""
          style={{ height: 300, display: 'block' }}
        />
      }
    >
      <span>{metadata.data.title}</span>
    </Tooltip>
  )
}

export default function TankoubonEdit() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { tankId = '' } = useParams<{ tankId: string }>()
  const tankoubon = useTankoubon(tankId)

  if (tankoubon.isLoading) {
    return (
      <div className="ido" style={{ textAlign: 'center', maxWidth: 800, margin: '10px auto', color: 'var(--theme-muted)' }}>
        {t('Loading library…')}
      </div>
    )
  }

  if (tankoubon.isError || !tankoubon.data) {
    return (
      <div className="ido" style={{ textAlign: 'center', maxWidth: 800, margin: '10px auto', display: 'flex', flexDirection: 'column', gap: 12, alignItems: 'center' }}>
        <p className="text-red-500">
          {t('Failed to load archives: {{error}}', { error: String(tankoubon.error) })}
        </p>
        <input
          className="stdbtn"
          type="button"
          value={t('Return to Library') ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    )
  }

  // Keyed by tankId so navigating between two different tankoubons' edit pages remounts this
  // form with fresh initial state, rather than needing an effect to re-sync it.
  return <TankoubonForm key={tankId} tankId={tankId} tankoubon={tankoubon.data} />
}

function TankoubonForm({ tankId, tankoubon }: { tankId: string; tankoubon: TankoubonMetadata }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  // Matches this page's own real heading text below ("Editing %1 (Tankoubon)") — no legacy
  // equivalent to cross-check against (Tankoubon editing is additive to this rewrite), so this
  // just keeps the tab title and the on-page heading in sync with each other.
  useDocumentTitle(t('Editing %1 (Tankoubon)').replace('%1', tankoubon.name))
  const updateTankoubon = useUpdateTankoubon(tankId)
  const deleteTankoubon = useDeleteTankoubon()
  const addToTankoubon = useAddToTankoubon(tankId)
  const stats = useStats(2)
  // Same source/shape as `Edit.tsx`'s own `tagSuggestions` (every tag used at least twice across
  // the library) — this page's Tags field now uses the same `TagInput` chip editor as the
  // archive edit page, not a plain textarea.
  const tagSuggestions = (stats.data ?? []).map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text))

  const [name, setName] = useState(tankoubon.name)
  const [summary, setSummary] = useState(tankoubon.summary)
  const [tags, setTags] = useState(tankoubon.tags)
  const [archives, setArchives] = useState(tankoubon.archives)
  const [newArchiveId, setNewArchiveId] = useState('')
  const [archiveSearchOpen, setArchiveSearchOpen] = useState(false)

  // Debounced so the title-search dropdown below doesn't fire one request per keystroke —
  // additive on top of the raw-ID input, which still works unchanged (see `addArchiveId`).
  const [debouncedQuery, setDebouncedQuery] = useState('')
  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedQuery(newArchiveId.trim()), 250)
    return () => clearTimeout(timeout)
  }, [newArchiveId])
  const archiveSearch = useSearch({ filter: debouncedQuery, enabled: debouncedQuery.length > 0 })
  // Excludes archives already in this Tankoubon, and any synthetic Tankoubon-aggregate rows the
  // search endpoint can return (`archive_count !== null`, matching `ArchiveMetadata`'s own doc
  // comment) — a Tankoubon can't usefully contain another Tankoubon. Capped at 15, same as the
  // Library search bar's own tag-autocomplete dropdown (`Library/index.tsx`'s `tagSuggestions`)
  // — the underlying `/search` endpoint's own page size (100) is far too many to usefully scroll
  // through in a suggestion dropdown.
  const archiveSearchResults = (archiveSearch.data?.data ?? [])
    .filter((a) => a.archive_count === null && !archives.includes(a.arcid))
    .slice(0, 15)

  // Legacy's own `Edit.saveMetadata` (`edit.js`) shows a "Metadata saved!" toast on every
  // successful save via `Server.callAPIBody`'s built-in success-message handling — this port's
  // `updateTankoubon` doesn't have that generic per-call toasting, so it's shown explicitly here.
  async function handleSave() {
    await updateTankoubon.mutateAsync({ metadata: { name, summary, tags } })
    toast({ heading: t('Metadata saved!') ?? undefined, icon: 'success' })
  }

  // Real legacy's own `edit.html.tt2` (`is_tank` branch) reorders this list via drag (`Sortable.
  // min.js`, `.drag-handle`), not up/down buttons — reusing `SortableList` (already used by
  // `SortablePluginGroup.tsx`) rather than the earlier button-based reorder this page started
  // with. The dropped order becomes the Tankoubon's own volume order (`archives`, order-
  // significant), persisted the same way `moveArchive` used to.
  function handleReorder(next: string[]) {
    setArchives(next)
    updateTankoubon.mutate({ archives: next })
  }

  function removeArchive(id: string) {
    const next = archives.filter((a) => a !== id)
    setArchives(next)
    updateTankoubon.mutate({ archives: next })
  }

  async function handleDelete() {
    await deleteTankoubon.mutateAsync(tankId)
    navigate(routes.library())
  }

  // Shared by the raw-ID "Add" button and picking a row from the title-search dropdown below —
  // same mutation either way, just a different source for the ID.
  async function addArchiveId(archiveId: string) {
    await addToTankoubon.mutateAsync(archiveId)
    setArchives((prev) => [...prev, archiveId])
    setNewArchiveId('')
    setArchiveSearchOpen(false)
  }

  async function handleAddArchive() {
    const id = newArchiveId.trim()
    if (!id) return
    await addArchiveId(id)
  }

  return (
    <div className="ido" style={{ textAlign: 'center', maxWidth: 800, margin: '10px auto' }}>
      <h2 className="ih" style={{ textAlign: 'center' }}>
        {t('Editing %1 (Tankoubon)').replace('%1', tankoubon.name)}
      </h2>

      <form
        autoComplete="off"
        style={{ width: '98%', maxWidth: 700, margin: '0 auto', fontSize: '8pt' }}
        onSubmit={(e) => e.preventDefault()}
      >
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10, textAlign: 'left' }}>
          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'center', gap: 6 }}>
            <span>{t('Title:')}</span>
            <input
              className="stdinput"
              type="text"
              style={{ width: '100%', maxWidth: 'none' }}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>{t('Summary:')}</span>
            <textarea
              className="stdinput"
              style={{ width: '100%', maxWidth: 'none', minHeight: 72, boxSizing: 'border-box' }}
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
            />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>
              {t('Tags')} <span style={{ fontSize: '6pt' }}>{t('(separated by hyphens, i.e : tag1, tag2)')}</span> :
            </span>
            <TagInput value={tags} onChange={setTags} suggestions={tagSuggestions} />
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'start', gap: 6 }}>
            <span>{t('Archives:')}</span>
            {/* `SortableList`'s own `DndContext`/`SortableContext` render transparently (no DOM
                wrapper of their own), so without this wrapping div each row's bare element would
                land as a direct child of this `grid` container and get independently
                auto-placed instead of staying confined to this one column — a real observed bug
                (the two rows ended up at unrelated x-positions instead of stacked in column 2). */}
            <div style={{ width: '100%' }}>
              <SortableList
                items={archives}
                getId={(archiveId) => archiveId}
                onReorder={handleReorder}
                renderItem={(archiveId, dragHandleProps) => (
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8, padding: '2px 0' }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8, overflow: 'hidden' }}>
                      <span
                        {...dragHandleProps.attributes}
                        {...dragHandleProps.listeners}
                        style={{
                          flexShrink: 0,
                          display: 'flex',
                          cursor: dragHandleProps.isDragging ? 'grabbing' : 'grab',
                          touchAction: 'none',
                          opacity: 0.6,
                        }}
                      >
                        <i className="fa fa-grip-vertical" aria-hidden="true"></i>
                      </span>
                      <span style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
                        <ArchiveTitle archiveId={archiveId} />
                      </span>
                    </div>
                    <div style={{ display: 'flex', gap: 6, flexShrink: 0, alignItems: 'center' }}>
                      {/* `.stdbtn`'s own legacy CSS sets `min-width: 150px` (sized for the
                          standalone, spread-out action rows below, e.g. "Save Metadata") —
                          packed into one compact per-archive row instead would demand way more
                          room than available and get unevenly flex-shrunk, a real observed bug
                          (each row's button widths came out different and inconsistent).
                          Overriding to a small fixed width here keeps `.stdbtn`'s color/border/
                          font but not that assumption; legacy's own equivalent row isn't
                          `.stdbtn` at all, just bare icon links with no width constraint of
                          their own. */}
                      <button
                        type="button"
                        className="stdbtn"
                        onClick={() => navigate(routes.edit(archiveId))}
                        style={{ minWidth: 32 }}
                      >
                        {t('Edit')}
                      </button>
                      <button
                        type="button"
                        className="stdbtn"
                        onClick={() => removeArchive(archiveId)}
                        title={t('Remove from Tankoubon') ?? undefined}
                        style={{ minWidth: 32 }}
                      >
                        ✕
                      </button>
                    </div>
                  </div>
                )}
              />
            </div>
          </div>

          <div style={{ display: 'grid', gridTemplateColumns: '120px 1fr', alignItems: 'center', gap: 6 }}>
            <span>{t('Add Archive to Tankoubon:')}</span>
            <div style={{ display: 'flex', gap: 6 }}>
              {/* The raw-ID paste-and-click-Add flow below is unchanged; this additionally
                  live-searches by title as the user types (debounced, `archiveSearch` above) and
                  offers a click-to-add dropdown with a thumbnail preview per match — for anyone
                  who doesn't already have the 40-char ID copied. */}
              <span style={{ position: 'relative', width: '100%' }}>
                <input
                  className="stdinput"
                  type="text"
                  // The surrounding `<form autoComplete="off">` doesn't reliably stop Chrome's
                  // own value-history autofill dropdown on a plain text input — it needs
                  // `autoComplete="off"` set directly on the field itself. That native dropdown
                  // is browser-chrome UI, not page DOM, so it can never get a hover-thumbnail
                  // like the custom `PopupMenu` search dropdown below can; turning it off avoids
                  // the two dropdowns visually fighting each other instead.
                  autoComplete="off"
                  // `.stdinput`'s own legacy height (18px) is 3px shorter than `.stdbtn`'s
                  // (21px, border-box, both already `boxSizing: 'border-box'` from the theme
                  // CSS) — matches the button next to it exactly rather than sitting visibly
                  // shorter.
                  style={{ width: '100%', maxWidth: 'none', height: 21, boxSizing: 'border-box' }}
                  value={newArchiveId}
                  onChange={(e) => {
                    setNewArchiveId(e.target.value)
                    // Also needed here, not just `onFocus` below — `addArchiveId` closes the
                    // dropdown after a click-to-add, but focus never actually leaves the input
                    // (the dropdown item's `onMouseDown` calls `preventDefault()` specifically to
                    // stop that), so `onFocus` never fires again on subsequent keystrokes. A real,
                    // confirmed bug: after adding one archive via the dropdown, typing a new query
                    // right after produced zero visible results despite the search request itself
                    // firing and returning real matches (`archiveSearchOpen` just stayed `false`).
                    setArchiveSearchOpen(true)
                  }}
                  onFocus={() => setArchiveSearchOpen(true)}
                  onBlur={() => setTimeout(() => setArchiveSearchOpen(false), 150)}
                  onKeyDown={(e) => {
                    if (e.key === 'Escape') setArchiveSearchOpen(false)
                  }}
                  placeholder={t('Archive ID (40-character long)') ?? undefined}
                />
                {archiveSearchOpen && archiveSearchResults.length > 0 && (
                  <PopupMenu
                    portal={false}
                    // `PopupMenu`'s own `m-[.3em]` class (a deliberate small gap for its usual
                    // context-menu use) otherwise offsets this flush-to-the-input dropdown a few
                    // px right/down from the input's real edge — a real, confirmed
                    // `getBoundingClientRect()` mismatch (input left 138.67 vs. menu left
                    // 140.86 at a 375px-wide viewport), not just a screenshot artifact.
                    // `boxSizing: 'border-box'` on top of that: `PopupMenu`'s `<ul>` has no
                    // box-sizing of its own (content-box default), so its `border: 1px solid`
                    // was rendering 1px *outside* the `minWidth: 100%` content box on each
                    // side — 2px wider overall than the input than it should be.
                    // `left: 1` (not `0`): `.stdinput`'s own legacy CSS margin is
                    // `4px 1px 0px` — a real 1px left margin that shifts the input's own border
                    // box 1px right of this wrapping `<span>`'s left edge (which has no margin
                    // of its own), confirmed via `getBoundingClientRect()` (input left 497 vs.
                    // an unadjusted `left: 0` menu at 496).
                    style={{
                      position: 'absolute',
                      top: '100%',
                      left: 1,
                      margin: 0,
                      zIndex: 1,
                      minWidth: '100%',
                      maxHeight: 320,
                      overflowY: 'auto',
                      boxSizing: 'border-box',
                    }}
                  >
                    {archiveSearchResults.map((a) => (
                      <PopupMenuItem
                        key={a.arcid}
                        onMouseDown={(e) => {
                          // Beats the input's own `onBlur` (fires first on mousedown), same
                          // reasoning as the Library search bar's own tag-autocomplete dropdown.
                          e.preventDefault()
                          void addArchiveId(a.arcid)
                        }}
                      >
                        <Tooltip
                          anchor="cursor"
                          wrapperStyle={{ display: 'inline' }}
                          label={
                            <img
                              src={`/api/archives/${a.arcid}/thumbnail?no_fallback=true`}
                              alt=""
                              style={{ height: 300, display: 'block' }}
                            />
                          }
                        >
                          <span>{a.title}</span>
                        </Tooltip>
                      </PopupMenuItem>
                    ))}
                  </PopupMenu>
                )}
              </span>
              <input
                className="stdbtn"
                type="button"
                style={{ minWidth: 32 }}
                value={t('Add') ?? undefined}
                onClick={() => void handleAddArchive()}
              />
            </div>
          </div>

          {/* Order matches real legacy exactly (`edit.html.tt2`'s `is_tank` branch: save / Delete
              Tankoubon / Read Tankoubon / Return to Library) — all four always shown, no
              conditional on `archives` being non-empty. "Read Tankoubon" navigates to the tank's
              own ID (`routes.reader(tankId)`, same as legacy's `#read-archive` handler navigating
              to `/reader?id=<the ID field's value>`, which for this branch is the tank ID, not any
              one member archive) — the reader route already resolves a Tankoubon ID into its
              member archives (`ArchiveCard.tsx` uses the same `routes.reader(id)` for both archive
              and Tankoubon cards in the Library grid).
              The label itself is a deliberate departure from legacy, though: legacy reuses the
              exact same "Save Metadata" wording for both archive and Tankoubon edit pages (a
              single shared `#save-metadata` button in one template) — this page uses "Update"
              instead, since a Tankoubon's own archive add/remove/reorder actions already persist
              immediately on each action (unlike legacy, which only saves them as part of this same
              click), so "Save Metadata" undersells what's already been saved by the time this
              button exists purely to commit name/summary/tags. */}
          <div style={{ display: 'flex', flexWrap: 'wrap', justifyContent: 'center', gap: 8, marginTop: 10 }}>
            <input
              className="stdbtn"
              type="button"
              value={t('Update') ?? undefined}
              onClick={() => void handleSave()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Delete Tankoubon') ?? undefined}
              onClick={() => void handleDelete()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Read Tankoubon') ?? undefined}
              onClick={() => navigate(routes.reader(tankId))}
            />
            <input
              className="stdbtn"
              type="button"
              value={t('Return to Library') ?? undefined}
              onClick={() => navigate(routes.library())}
            />
          </div>
        </div>
      </form>
    </div>
  )
}
