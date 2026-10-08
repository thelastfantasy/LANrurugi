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
