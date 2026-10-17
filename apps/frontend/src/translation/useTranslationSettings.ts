// Loads the server-stored translation settings for the reader, and lets the reader's own toolbar
// flip the per-archive/per-Tankoubon enabled switch without a trip to the settings page
// (provider/key/language/etc. are still configured there — this only ever touches the one thing a
// reader mid-session actually wants: on or off for *this* book).
//
// Its own file rather than living inside the Reader page component, per constitution Principle VII
// (a page's `index.tsx`/page file exports only that page's component).
//
// Returns `null` until loaded, and on failure. Both mean the same thing to every caller — "don't
// attempt translation" — which keeps a settings-fetch problem from ever affecting reading (FR-020).

import { useCallback, useEffect, useState } from "react"

import {
  fetchTranslationScope,
  fetchTranslationSettings,
  updateTranslationScope,
  updateTranslationSettings,
} from "./api"
import type { TranslationScope, TranslationSettings } from "./types"

export function useTranslationSettings(
  archiveId: string | null | undefined,
): [
  TranslationSettings | null,
  TranslationScope | null,
  () => void,
  (language: string | null) => void,
] {
  const [settings, setSettings] = useState<TranslationSettings | null>(null)
  const [scope, setScope] = useState<TranslationScope | null>(null)

  useEffect(() => {
    let cancelled = false

    void fetchTranslationSettings()
      .then((loaded) => {
        if (!cancelled) setSettings(loaded)
      })
      .catch(() => {
        // Left as `null` — reading proceeds exactly as it would with translation off.
      })

    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    if (!archiveId) return

    let cancelled = false

    void fetchTranslationScope(archiveId)
      .then((loaded) => {
        if (!cancelled) setScope(loaded)
      })
      .catch(() => {
        // Left as whatever it was — same "don't attempt translation" fallback as a
        // settings-fetch failure once resolved below against the *current* `archiveId`.
      })

    return () => {
      // Stale scope from the previous archive must never be shown against a new one — cleared on
      // every archiveId change (including to `null`), not just re-set once a fetch resolves.
      cancelled = true
      setScope(null)
    }
  }, [archiveId])

  const toggleEnabled = useCallback(() => {
    if (!archiveId) return
    setScope((current) => {
      const nextEnabled = !(current?.enabled ?? false)
      // Optimistic: the toolbar button should feel instant. A failed PUT leaves the server's own
      // value unchanged, so a subsequent read (e.g. reopening the reader) self-corrects — this is
      // the same "don't let a settings round-trip affect reading" posture the hook already takes.
      void updateTranslationScope(archiveId, nextEnabled).catch(() => {})
      return { enabled: nextEnabled, scope: current?.scope ?? "archive" }
    })
  }, [archiveId])

  const setTargetLanguage = useCallback((language: string | null) => {
    setSettings((current) => {
      if (!current) return current
      const next = { ...current, targetLanguage: language }
      void updateTranslationSettings({ targetLanguage: language }).catch(() => {})
      return next
    })
  }, [])

  return [settings, scope, toggleEnabled, setTargetLanguage]
}
