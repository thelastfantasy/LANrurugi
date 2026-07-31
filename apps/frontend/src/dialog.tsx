import { useEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'

import { useStats } from './api/hooks'
import { PopupMenu, PopupMenuItem } from './components/PopupMenu'
import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from './theme'

// Real, themed replacements for `window.prompt`/`window.confirm` — same call shape as those
// (`await promptDialog(message, defaultValue)` / `await confirmDialog(message)`, both resolving
// to what the plain browser function would have returned: the entered string or `null` if
// cancelled/empty for a prompt, a boolean for a confirm) so existing call sites need only add
// `await` and drop the `window.` prefix, not a structural rewrite. A *native* `window.prompt`/
// `window.confirm` is an unstyled OS dialog outside the page's own DOM/CSS entirely — it happens
// to pick up a vaguely similar red/cream palette on some Linux desktop themes purely by
// coincidence of that OS theme, never because of anything this app's own CSS controls. Legacy
// itself never used the native versions either — its real popups are SweetAlert2 (`LRR.showPopUp`/
// `Swal.fire`), which is exactly what `.swal2-popup`/`.swal2-actions>.stdbtn` (real classes already
// vendored per-theme, e.g. `~/LANraragi/public/themes/g.css:643,220`) exist to style — this module
// reuses those same classes rather than either the native dialogs or a new SweetAlert2 dependency,
// matching `Library.tsx`'s own pre-existing `DeleteConfirmDialog` in spirit (a from-scratch themed
// popup) but as a shared, promise-based module usable from any file instead of one bespoke
// component wired into one page's own local state.
//
// Architecture mirrors `toast.tsx` exactly: a module-level "current request" state that any file
// can push into via a plain function call, rendered by one `<DialogHost />` mounted once in
// `App.tsx` (matching `toast.tsx`'s own `<ToastContainer>` convention) — not a React hook, since
// call sites need this to work from plain event handlers exactly like `window.prompt` did, not
// only from inside a component's own render.

export type NewCategoryResult = { name: string; isDynamic: boolean; search: string }

type DialogRequest =
  | {
      kind: 'prompt'
      message: string
      defaultValue: string
      resolve: (value: string | null) => void
    }
  | {
      kind: 'confirm'
      message: string
      resolve: (value: boolean) => void
    }
  | {
      kind: 'newCategory'
      resolve: (value: NewCategoryResult | null) => void
    }

let currentRequest: DialogRequest | null = null
let listeners: (() => void)[] = []

function setRequest(request: DialogRequest | null) {
  currentRequest = request
  listeners.forEach((l) => l())
}

/** Drop-in replacement for `window.prompt(message, defaultValue)` — resolves to the entered
 * string, or `null` if cancelled (matches the native function's own return shape exactly, so a
 * call site's existing `if (title && title.trim() !== '')` guard keeps working unchanged). */
export function promptDialog(message: string, defaultValue = ''): Promise<string | null> {
  return new Promise((resolve) => {
    setRequest({ kind: 'prompt', message, defaultValue, resolve })
  })
}

/** Drop-in replacement for `window.confirm(message)`. */
export function confirmDialog(message: string): Promise<boolean> {
  return new Promise((resolve) => {
    setRequest({ kind: 'confirm', message, resolve })
  })
}

/** One combined "New Category" dialog (name + a static/dynamic tab switcher, with a predicate
 * field that only appears in dynamic mode) shared by every quick-create entry point (the Reader
 * overview overlay, the Upload page, and — in spirit — `Categories.tsx`'s own longer-standing
 * pair of "New Static/New Dynamic" buttons), rather than two separate single-purpose buttons each
 * call site would otherwise have to lay out and wire up itself. Resolves `null` if cancelled. */
export function newCategoryDialog(): Promise<NewCategoryResult | null> {
  return new Promise((resolve) => {
    setRequest({ kind: 'newCategory', resolve })
  })
}

// Same fragment-matching/sort rule as `Library.tsx`'s own search-bar autocomplete
// (`loadTagSuggestions`): only the piece after the last `,`/`-`/whitespace, case-insensitive
// substring, sorted by tag weight descending. Used for the dynamic-category predicate field,
// whose value is itself a LANraragi search-query string, not a plain name.
function TagSearchField({
  id,
  value,
  onChange,
  autoFocus,
  onEnter,
}: {
  id: string
  value: string
  onChange: (next: string) => void
  autoFocus: boolean
  onEnter: () => void
}) {
  const stats = useStats()
  const [open, setOpen] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)

  const currentFragment = value.match(/[^,\s-]*$/)?.[0] ?? ''
  const suggestions = useMemo(() => {
    if (!currentFragment) return []
    const needle = currentFragment.toLowerCase()
    return (stats.data ?? [])
      .map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text))
      .filter((label) => label.toLowerCase().includes(needle))
      .slice(0, 15)
  }, [stats.data, currentFragment])

  return (
    <span style={{ position: 'relative', display: 'block' }}>
      <input
        ref={inputRef}
        id={id}
        type="text"
        className="stdinput"
        style={{ width: '100%', boxSizing: 'border-box' }}
        value={value}
        autoComplete="off"
        autoFocus={autoFocus}
        onChange={(e) => {
          onChange(e.target.value)
          setOpen(true)
        }}
        onFocus={() => setOpen(true)}
        onBlur={() => setTimeout(() => setOpen(false), 150)}
        onKeyDown={(e) => {
          if (e.key === 'Enter') onEnter()
          if (e.key === 'Escape') setOpen(false)
        }}
      />
      {open && suggestions.length > 0 && (
        <PopupMenu portal={false} style={{ position: 'absolute', top: '100%', left: 0, zIndex: Z_OVERLAY_CONTENT, minWidth: '100%', maxHeight: 180, overflowY: 'auto' }}>
          {suggestions.map((label) => (
            <PopupMenuItem
              key={label}
              onMouseDown={(e) => {
                e.preventDefault()
                onChange(`${value.replace(/[^,\s-]*$/, '')}${label}`)
                setOpen(false)
                inputRef.current?.focus()
              }}
            >
              {label}
            </PopupMenuItem>
          ))}
        </PopupMenu>
      )}
    </span>
  )
}

function NewCategoryForm({ onSubmit, onCancel }: { onSubmit: (value: NewCategoryResult) => void; onCancel: () => void }) {
  const { t } = useTranslation()
  const [name, setName] = useState('')
  const [isDynamic, setIsDynamic] = useState(false)
  const [search, setSearch] = useState('')
  const nameRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    nameRef.current?.select()
  }, [])

  function submit() {
    if (!name.trim()) return
    onSubmit({ name: name.trim(), isDynamic, search: isDynamic ? search : '' })
  }

  return (
    <div onKeyDown={(e) => e.key === 'Escape' && onCancel()}>
      <p style={{ fontWeight: 'bold', margin: '0 0 12px' }}>{t('Enter a name for the new category')}</p>
      <input
        ref={nameRef}
        type="text"
        className="stdinput"
        style={{ width: '100%', boxSizing: 'border-box', marginBottom: 12 }}
        value={name}
        placeholder={t('My Category') ?? undefined}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && !isDynamic) submit()
        }}
      />
      {/* Segmented tab switcher — reuses `favtag-btn`/`.toggled`, the same pill-button-row pattern
          `Library.tsx`'s category filter bar already uses for a mutually-exclusive choice, rather
          than native radio inputs (visually inconsistent with the rest of the themed UI). */}
      <div role="tablist" style={{ display: 'flex', gap: 4, marginBottom: 12 }}>
        <button
          type="button"
          role="tab"
          aria-selected={!isDynamic}
          className={`favtag-btn${!isDynamic ? ' toggled' : ''}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(false)}
        >
          {t('Static Category')}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={isDynamic}
          className={`favtag-btn${isDynamic ? ' toggled' : ''}`}
          style={{ flex: 1 }}
          onClick={() => setIsDynamic(true)}
        >
          {t('Dynamic Category')}
        </button>
      </div>
      {isDynamic && (
        <div style={{ textAlign: 'left', marginBottom: 12 }}>
          <label htmlFor="new-category-search" style={{ display: 'block', fontSize: 13, marginBottom: 4 }}>
            {t('Search Predicate')}
          </label>
          <TagSearchField id="new-category-search" value={search} onChange={setSearch} autoFocus={false} onEnter={submit} />
        </div>
      )}
      <div className="swal2-actions" style={{ display: 'flex', justifyContent: 'center', gap: 8 }}>
        <input type="button" className="stdbtn" value={t('Cancel') ?? 'Cancel'} onClick={onCancel} />
        <input type="button" className="stdbtn" value={t('OK') ?? 'OK'} onClick={submit} />
      </div>
    </div>
  )
}

/** Mounted once, app-wide (see `App.tsx`) — matches `toast.tsx`'s own `<ToastContainer>`
 * convention exactly: a single always-present host that any file's plain `promptDialog`/
 * `confirmDialog`/`newCategoryDialog` call can push a request into, regardless of which component
 * tree is currently mounted where. */
export function DialogHost() {
  const { t } = useTranslation()
  const [, forceUpdate] = useState(0)
  const inputRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    const listener = () => forceUpdate((n) => n + 1)
    listeners.push(listener)
    return () => {
      listeners = listeners.filter((l) => l !== listener)
    }
  }, [])

  const request = currentRequest

  useEffect(() => {
    if (request?.kind === 'prompt') inputRef.current?.select()
  }, [request])

  if (!request) return null

  function close() {
    setRequest(null)
  }

  function submitPrompt() {
    if (request?.kind !== 'prompt') return
    const value = inputRef.current?.value ?? ''
    request.resolve(value)
    close()
  }

  function cancelPrompt() {
    if (request?.kind !== 'prompt') return
    request.resolve(null)
    close()
  }

  function confirmYes() {
    if (request?.kind !== 'confirm') return
    request.resolve(true)
    close()
  }

  function confirmNo() {
    if (request?.kind !== 'confirm') return
    request.resolve(false)
    close()
  }

  function cancelNewCategory() {
    if (request?.kind !== 'newCategory') return
    request.resolve(null)
    close()
  }

  function submitNewCategory(value: NewCategoryResult) {
    if (request?.kind !== 'newCategory') return
    request.resolve(value)
    close()
  }

  if (request.kind === 'newCategory') {
    return createPortal(
      <>
        <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: 'rgba(0,0,0,0.4)' }} onClick={cancelNewCategory} />
        <div
          role="dialog"
          aria-modal="true"
          className="swal2-popup"
          style={{
            position: 'fixed',
            top: '50%',
            left: '50%',
            transform: 'translate(-50%, -50%)',
            zIndex: Z_OVERLAY_CONTENT,
            display: 'block',
            width: 360,
            padding: 20,
            textAlign: 'center',
            borderRadius: '.2em',
            boxShadow: '0 2px 10px rgba(0,0,0,.4)',
          }}
        >
          <NewCategoryForm onSubmit={submitNewCategory} onCancel={cancelNewCategory} />
        </div>
      </>,
      document.body,
    )
  }

  const onCancel = request.kind === 'prompt' ? cancelPrompt : confirmNo
  const onConfirm = request.kind === 'prompt' ? submitPrompt : confirmYes

  return createPortal(
    <>
      <div style={{ position: 'fixed', inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: 'rgba(0,0,0,0.4)' }} onClick={onCancel} />
      <div
        role="dialog"
        aria-modal="true"
        className="swal2-popup"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: Z_OVERLAY_CONTENT,
          display: 'block',
          width: 360,
          padding: 20,
          textAlign: 'center',
          borderRadius: '.2em',
          boxShadow: '0 2px 10px rgba(0,0,0,.4)',
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') onCancel()
          if (e.key === 'Enter' && request.kind === 'confirm') onConfirm()
        }}
      >
        <p style={{ fontWeight: 'bold', margin: '0 0 12px' }}>{request.message}</p>
        {request.kind === 'prompt' && (
          <input
            ref={inputRef}
            type="text"
            className="stdinput"
            style={{ width: '100%', boxSizing: 'border-box', marginBottom: 12 }}
            defaultValue={request.defaultValue}
            placeholder={request.defaultValue || undefined}
            autoFocus
            onKeyDown={(e) => {
              if (e.key === 'Enter') onConfirm()
            }}
          />
        )}
        <div className="swal2-actions" style={{ display: 'flex', justifyContent: 'center', gap: 8 }}>
          <input type="button" className="stdbtn" value={t('Cancel') ?? 'Cancel'} onClick={onCancel} />
          <input type="button" className="stdbtn" value={t('OK') ?? 'OK'} onClick={onConfirm} />
        </div>
      </div>
    </>,
    document.body,
  )
}
