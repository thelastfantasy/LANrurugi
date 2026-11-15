import "@testing-library/jest-dom/vitest"

// jsdom ships no `ResizeObserver`, and components that measure their own box (the preview's
// truncation tooltips) construct one on mount. A no-op keeps them renderable; anything asserting on
// observed resizes would need a real implementation.
if (!("ResizeObserver" in globalThis)) {
  class NoopResizeObserver implements ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  globalThis.ResizeObserver = NoopResizeObserver as unknown as typeof ResizeObserver
}

// jsdom has no `matchMedia` either; `useIsNarrowViewport` reads it on mount. Default to a wide
// viewport that never changes; tests about the narrow layout stub their own (`vi.stubGlobal`).
if (typeof window !== "undefined" && !window.matchMedia) {
  window.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  })) as typeof window.matchMedia
}
