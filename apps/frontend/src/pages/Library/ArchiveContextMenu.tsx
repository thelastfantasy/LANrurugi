import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router-dom'

import type { ArchiveMetadata } from '../../api/types'
import { PopupMenu, PopupMenuItem, PopupMenuSeparator, useMenuPalette } from '../../components/PopupMenu'
import RatingWidget from '../../components/RatingWidget'
import { splitTagsByNamespace } from '../../lib/tagFormat'
import { routes } from '../../routes'
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from '../../theme'
import { toast } from '../../toast'
import { type ContextMenuState, isTankoubonId } from './shared'

/** Ports legacy's own right-click menu (`~/LANraragi/public/js/mod/index_contextmenu.js`) — same
 * action set and same login-gating (Edit/Delete/Rating/Category only shown when `useLoginStatus`
 * reports logged in). Built entirely from `PopupMenu`/`PopupMenuItem`/`PopupMenuSeparator`
 * (`components/PopupMenu.tsx`) — a from-scratch React component styled with Tailwind + this app's
 * own `MENU_PALETTE` colour table, matching each of legacy's 5 real themes without depending on
 * any menu-plugin's markup or CSS. Closes on any outside click or right-click. */
export function ArchiveContextMenu({
  state,
  categories,
  loggedIn,
  liveArchives,
  onClose,
  onToggleCategory,
  onDelete,
  onOpen,
  onRatingChange,
  onToggleSelection,
  isSelected,
  onSetProgress,
}: {
  state: ContextMenuState
  categories: { id: string; name: string; search: string | null; archives: string[] }[] | undefined
  loggedIn: boolean
  /** The live, refetch-synced search results (`shown` in the parent) — `state.archive` itself is
   * a one-time snapshot taken at right-click time and never updated, which used to be harmless
   * (every action that changed the archive also closed the menu immediately) but broke once the
   * rating row became a persistent, stay-open-after-click control: clicking a star correctly
   * updated the archive in Redis, but the menu kept rendering the stale pre-click tags until
   * closed and reopened. Looked up by ID so the rating row (and anything else keying off
   * `archive.tags`) reflects the real just-saved value without needing its own separate refetch. */
  liveArchives: ArchiveMetadata[]
  onClose: () => void
  onToggleCategory: (categoryId: string, archiveId: string, currentlyIn: boolean) => void
  onDelete: (archiveId: string, isTank: boolean) => void
  onOpen: (id: string) => void
  onRatingChange: (archiveId: string, isTank: boolean, rating: string | null) => void
  onToggleSelection: (id: string) => void
  isSelected: boolean
  /** "Mark as Read"/"Mark as Unread" — the actual mutation lives in the parent `Library`
   * component, not here, because `onClose()` (called first, on the same click) unmounts this
   * whole menu immediately; a `useMutation` instance owned by *this* component would have its
   * observer torn down before the mutation's async response ever arrives, silently dropping
   * whatever `onSuccess` callback was passed to that particular `mutate()` call (TanStack Query's
   * own `hasListeners()` guard on delivering per-call `mutate(vars, { onSuccess })` callbacks — a
   * real, live-confirmed bug: the write to Redis genuinely succeeded and the main grid's `invalidate
   * Queries`-driven refetch picked it up fine since that mutation is defined in the *parent*, but
   * a second effect meant to also refresh the On Deck carousel never fired at all, because it was
   * wired through this component's own now-torn-down mutation instance instead). */
  onSetProgress: (archiveId: string, page: number) => void
}) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { x, y } = state
  const archive = liveArchives.find((a) => a.arcid === state.archive.arcid) ?? state.archive
  const isTank = isTankoubonId(archive.arcid)
  const staticCategories = (categories ?? []).filter((c) => !c.search)
  const [categoryMenuOpen, setCategoryMenuOpen] = useState(false)
  const palette = useMenuPalette()

  // Submenu opens on hover (matching legacy's `jquery-contextMenu`, and standard desktop
  // context-menu behavior generally) rather than click. A short close delay absorbs the mouse
  // briefly leaving the trigger row while crossing the gap into the submenu itself.
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  function openSubmenu(which: 'category') {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current)
    setCategoryMenuOpen(which === 'category')
  }
  function scheduleCloseSubmenus() {
    if (closeTimerRef.current) clearTimeout(closeTimerRef.current)
    closeTimerRef.current = setTimeout(() => {
      setCategoryMenuOpen(false)
    }, 200)
  }

  function copyLink() {
    const url = `${window.location.origin}${routes.reader(archive.arcid)}`
    navigator.clipboard
      .writeText(url)
      .then(() => toast({ heading: t('Link copied to clipboard!') ?? undefined, icon: 'info', hideAfter: 3000 }))
      .catch(() => toast({ heading: t('Failed to copy link.') ?? undefined, icon: 'error' }))
  }

  return (
    <>
      {/* Full-viewport transparent overlay — the standard "click outside to dismiss" pattern for
          a positioned popup, cheaper than a document-level listener + ref check. */}
      <div
        style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP }}
        onClick={onClose}
        onContextMenu={(e) => {
          e.preventDefault()
          onClose()
        }}
      />
      <PopupMenu style={{ position: 'fixed', top: y, left: x, zIndex: Z_OVERLAY_CONTENT }}>
        {loggedIn && (
          <>
            {/* A compact icon-only row at the very top of the menu (Firefox's own right-click
                menu puts Back/Forward/Reload/Bookmark the same way) rather than a full-width
                "Add Rating" row that opens a whole separate hover submenu — the star widget's own
                click targets are already precise enough that a submenu was pure overhead. Not a
                `PopupMenuItem` (no hover-highlight-the-whole-row/click-closes-menu behavior makes
                sense for a row of independent controls). */}
            <li
              className="flex items-center justify-center gap-1 px-2 pt-1"
              style={{ paddingBottom: '.45em', borderBottom: `1px solid ${palette.separator}`, marginBottom: '.35em' }}
            >
              <RatingWidget
                archiveId={archive.arcid}
                tags={archive.tags}
                size={16}
                onChange={(nextTags) => {
                  const tagsByNamespace = splitTagsByNamespace(nextTags)
                  const rating = tagsByNamespace.rating?.[0] ?? null
                  onRatingChange(archive.arcid, isTank, rating)
                }}
              />
            </li>
          </>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose()
            onOpen(archive.arcid)
          }}
        >
          <i className="fa fa-book-open" style={{ width: 18 }}></i> {t('Read')}
        </PopupMenuItem>
        {/* Not offered on a Tankoubon — it's an aggregate container with no single `progress`/
            `pagecount` of its own (each member archive tracks its own separately), so "mark this
            one thing as read" doesn't have a single well-defined target the way it does for a
            plain archive. Toggles on `progress > 0` (any progress at all counts as "not unread"),
            not the 85%-complete threshold `hidecompleted`/On Deck use elsewhere — those answer a
            different question ("is this basically finished, worth hiding from an in-progress
            list") than this menu item's own binary read/unread state. */}
        {!isTank && archive.pagecount > 0 && (
          <PopupMenuItem
            onClick={() => {
              onClose()
              onSetProgress(archive.arcid, archive.progress > 0 ? 0 : archive.pagecount)
            }}
          >
            <i className={`fa ${archive.progress > 0 ? 'fa-eye-slash' : 'fa-eye'}`} style={{ width: 18 }}></i>{' '}
            {archive.progress > 0 ? t('Mark as Unread') : t('Mark as Read')}
          </PopupMenuItem>
        )}
        {!isTank && (
          <PopupMenuItem
            onClick={() => {
              onClose()
              window.location.assign(`/api/archives/${archive.arcid}/download`)
            }}
          >
            <i className="fa fa-download" style={{ width: 18 }}></i> {t('Download')}
          </PopupMenuItem>
        )}
        <PopupMenuItem
          onClick={() => {
            onClose()
            copyLink()
          }}
        >
          <i className="fa fa-link" style={{ width: 18 }}></i> {t('Copy Link')}
        </PopupMenuItem>
        <PopupMenuItem
          onClick={() => {
            onClose()
            onToggleSelection(archive.arcid)
          }}
        >
          <i className="fa fa-check-square" style={{ width: 18 }}></i>{' '}
          {isSelected ? t('Remove from Selection') : t('Add to Selection')}
        </PopupMenuItem>
        {loggedIn && (
          <>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose()
                navigate(isTank ? routes.tankoubonEdit(archive.arcid) : routes.edit(archive.arcid))
              }}
            >
              <i className="fa fa-pen" style={{ width: 18 }}></i>{' '}
              {isTank ? t('Edit Tankoubon') : t('Edit Metadata')}
            </PopupMenuItem>
            <PopupMenuItem style={{ position: 'relative' }} onMouseEnter={() => openSubmenu('category')} onMouseLeave={scheduleCloseSubmenus}>
              <i className="fa fa-search-plus" style={{ width: 18 }}></i> {t('Add to Category')}
              {categoryMenuOpen && (
                <PopupMenu
                  portal={false}
                  style={{ position: 'absolute', left: '100%', top: 0, maxHeight: 220, overflowY: 'auto' }}
                  onMouseEnter={() => openSubmenu('category')}
                  onMouseLeave={scheduleCloseSubmenus}
                >
                  {staticCategories.length === 0 && (
                    <PopupMenuItem disabled>{t('No categories found.')}</PopupMenuItem>
                  )}
                  {staticCategories.map((c) => {
                    const currentlyIn = c.archives.includes(archive.arcid)
                    return (
                      <PopupMenuItem key={c.id} onClick={() => onToggleCategory(c.id, archive.arcid, currentlyIn)}>
                        <input type="checkbox" readOnly checked={currentlyIn} style={{ verticalAlign: 'middle' }} /> {c.name}
                      </PopupMenuItem>
                    )
                  })}
                </PopupMenu>
              )}
            </PopupMenuItem>
            <PopupMenuSeparator />
            <PopupMenuItem
              onClick={() => {
                onClose()
                onDelete(archive.arcid, isTank)
              }}
            >
              <i className="fa fa-trash" style={{ width: 18 }}></i> {t('Delete')}
            </PopupMenuItem>
          </>
        )}
      </PopupMenu>
    </>
  )
}

/** Styled delete-confirmation popup (legacy's `LRR.showPopUp` — a SweetAlert2 dialog), replacing
 * a plain `window.confirm`, with text that differs for a Tankoubon vs a plain archive (legacy's
 * own `ConfirmTankoubonDeletion`/`ConfirmArchiveDeletion` distinction). */
export function DeleteConfirmDialog({
  isTank,
  onConfirm,
  onCancel,
}: {
  isTank: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  return (
    <>
      <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: 'rgba(0,0,0,0.4)' }} onClick={onCancel} />
      <div
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: Z_OVERLAY_CONTENT,
          width: 360,
          padding: 20,
          textAlign: 'center',
          background: '#fff',
          border: '1px solid #bebebe',
          borderRadius: '.2em',
          boxShadow: '0 2px 5px rgba(0,0,0,.5)',
        }}
      >
        <i className="fa fa-exclamation-triangle fa-2x" style={{ color: '#d33' }} aria-hidden="true"></i>
        <p>
          {isTank
            ? t('This will delete this Tankoubon grouping (archives inside it are not deleted).')
            : t('This will delete both metadata and matching files from your system! Please use with caution.')}
        </p>
        <div style={{ display: 'flex', justifyContent: 'center', gap: 12, marginTop: 12 }}>
          <input type="button" className="stdbtn" value={t('Cancel') ?? undefined} onClick={onCancel} />
          <input
            type="button"
            className="stdbtn"
            style={{ background: '#d33', color: 'white' }}
            value={t('Yes, delete it') ?? undefined}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>
  )
}
