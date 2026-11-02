// Lifted state for `TranslationSection`, so `SettingsPage`'s single top-level save button can
// submit it alongside the rest of Phase 1's settings — matching every other section's own
// "state lives in SettingsPage, section is a pure controlled component" convention.
//
// Still a dedicated reducer (not five separate `useState`s the way `GlobalSection`'s primitives
// are) because `TranslationSettings` itself is one coherent server-stored object with one existing
// "patch a few fields" update shape (`update()` in the pre-lift version of this component) — a
// reducer keeps that patch semantics in one place instead of `SettingsPage` having to know how to
// merge translation-settings fields itself.

import { useReducer } from "react"

import { getLocalBackend } from "@/translation/settings"
import type { LocalBackendConfig, TranslationSettings } from "@/translation/types"

export interface TranslationSectionState {
  /** `null` while the independent `GET /translation/settings` fetch is still in flight — this
   * section renders nothing until it resolves, same as before the lift. */
  settings: TranslationSettings | null
  /** The last value actually confirmed by the server (right after `loaded`/`saved`) — kept
   * separate from `settings` (which tracks in-progress edits) purely so `SettingsPage`'s top-level
   * `isDirty` check has something stable to diff `settings` against, the same way it already diffs
   * every Phase 1 field against its own `settings.<field>` prop. */
  serverSettings: TranslationSettings | null
  category: "cloud" | "local"
  /** Device-local backend config — never sent to the server (research.md §8); persisted to
   * `localStorage` only at actual save time, not on every keystroke. */
  local: LocalBackendConfig
  /** Write-only; cleared immediately after a successful save, never retained once stored
   * server-side (FR-006). */
  apiKey: string
  /** Snapshot of `category`/`local` as they were read from `localStorage` at mount — same
   * dirty-check role `serverSettings` plays for `settings`, but for the two fields that never
   * round-trip through the server at all. */
  initialCategory: "cloud" | "local"
  initialLocal: LocalBackendConfig
}

export type TranslationSectionAction =
  | { kind: "loaded"; settings: TranslationSettings }
  | { kind: "patched"; patch: Partial<TranslationSettings> }
  | { kind: "categoryChanged"; category: "cloud" | "local" }
  | { kind: "localChanged"; local: LocalBackendConfig }
  | { kind: "apiKeyChanged"; apiKey: string }
  /** After a successful save: the server's own echo replaces `settings` (picks up anything it
   * normalized), and the write-only key input is cleared. */
  | { kind: "saved"; settings: TranslationSettings }

function reducer(state: TranslationSectionState, action: TranslationSectionAction): TranslationSectionState {
  switch (action.kind) {
    case "loaded":
      return { ...state, settings: action.settings, serverSettings: action.settings }
    case "patched":
      return state.settings ? { ...state, settings: { ...state.settings, ...action.patch } } : state
    case "categoryChanged":
      return { ...state, category: action.category }
    case "localChanged":
      return { ...state, local: action.local }
    case "apiKeyChanged":
      return { ...state, apiKey: action.apiKey }
    case "saved":
      return { ...state, settings: action.settings, serverSettings: action.settings, apiKey: "" }
  }
}

export function useTranslationSectionState(): [TranslationSectionState, React.Dispatch<TranslationSectionAction>] {
  return useReducer(reducer, undefined, () => {
    // The device-local backend lives in `localStorage`, which is readable synchronously at first
    // render — so it seeds initial state directly rather than being set from an effect (which would
    // render once with the wrong category, then again to correct it).
    const category = getLocalBackend() ? "local" : "cloud"
    const local = getLocalBackend() ?? { endpoint: "", model: "", identifier: "" }
    return {
      settings: null,
      serverSettings: null,
      category,
      local,
      apiKey: "",
      initialCategory: category,
      initialLocal: local,
    }
  })
}
