/** Which issues are ticked in the list (specs/018). Shared between the issues tab, where they are
 * selected, and the bundle tab, where the selection decides what gets packed. */

import { create } from "zustand"

interface IssueSelection {
  selected: Set<string>
  toggle: (fingerprint: string) => void
  /** Select all of these (or none of them) — what the header checkbox does. */
  setMany: (fingerprints: string[], on: boolean) => void
  /** Drop anything no longer listed (a level filter was switched off): a tick on a row that is not
   * on screen is invisible state, and it made the header checkbox read "8 of 5". */
  retain: (fingerprints: string[]) => void
  clear: () => void
}

export const useIssueSelection = create<IssueSelection>((set) => ({
  selected: new Set(),
  toggle: (fingerprint) =>
    set((state) => {
      const next = new Set(state.selected)
      if (next.has(fingerprint)) next.delete(fingerprint)
      else next.add(fingerprint)
      return { selected: next }
    }),
  setMany: (fingerprints, on) =>
    set((state) => {
      const next = new Set(state.selected)
      for (const fingerprint of fingerprints) {
        if (on) next.add(fingerprint)
        else next.delete(fingerprint)
      }
      return { selected: next }
    }),
  retain: (fingerprints) =>
    set((state) => {
      const visible = new Set(fingerprints)
      const next = new Set([...state.selected].filter((id) => visible.has(id)))
      return next.size === state.selected.size ? state : { selected: next }
    }),
  clear: () => set({ selected: new Set() }),
}))
