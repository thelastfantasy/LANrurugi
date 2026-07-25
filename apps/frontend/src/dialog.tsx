import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { useTranslation } from 'react-i18next'

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

/** Mounted once, app-wide (see `App.tsx`) — matches `toast.tsx`'s own `<ToastContainer>`
 * convention exactly: a single always-present host that any file's plain `promptDialog`/
 * `confirmDialog` call can push a request into, regardless of which component tree is currently
 * mounted where. */
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
