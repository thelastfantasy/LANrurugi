import { createContext, useContext } from "react"
import { createStore, useStore } from "zustand"

// Which sample owns the current touch-magnify session (shared fixed toggle button).
export interface TouchMagnifyState {
  active: {
    sampleKey: string
    showB: boolean
    toggleSide: () => void
    toggleOnRight: boolean
  } | null
  setActive: (active: TouchMagnifyState["active"]) => void
}

export function createTouchMagnifyStore() {
  return createStore<TouchMagnifyState>((set) => ({
    active: null,
    setActive: (active) => set({ active }),
  }))
}

export type TouchMagnifyStoreApi = ReturnType<typeof createTouchMagnifyStore>

export const TouchMagnifyStoreContext = createContext<TouchMagnifyStoreApi | null>(null)

export function useTouchMagnifyStore<T>(selector: (state: TouchMagnifyState) => T): T {
  const store = useContext(TouchMagnifyStoreContext)
  if (!store) {
    throw new Error("useTouchMagnifyStore must be used within a TouchMagnifyStoreContext.Provider")
  }
  return useStore(store, selector)
}
