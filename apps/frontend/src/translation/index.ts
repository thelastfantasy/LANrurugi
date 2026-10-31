// Barrel exports for Phase 2's on-page manga translation frontend
// (`specs/004-ocr-manga-translation`).
//
// The client-side half covers: the device-local backend selection and its precedence
// (`settings.ts`), the locally-hosted-backend direct call plus client-side compositing/cache
// (`localBackend.ts`, `composite.ts`, `cache.ts`, `localCache.ts`), the reader-facing
// not-yet-ready → ready swap (`readyPoller.ts`, `usePageTranslation.ts`), and look-ahead
// abandonment on navigate-away (`prefetchController.ts`).

export * from "./types"

export * from "./settings"

export * from "./cache"

export * from "./api"

export { compositePage, cssFontFamily } from "./composite"

export { getOrCompositePage, invalidateCachedPage } from "./localCache"

export {
  LocalBackendUnreachableError,
  reportLocalTranslation,
  translateWithLocalBackend,
} from "./localBackend"

export { pollUntilReady } from "./readyPoller"

export type { PollHandle } from "./readyPoller"

export { PrefetchController } from "./prefetchController"

export { usePageTranslation } from "./usePageTranslation"

export { default as TranslationLoadingIndicator } from "./components/LoadingIndicator"

export { default as LocalBackendGuidance } from "./components/LocalBackendGuidance"

export { default as UsagePanel } from "./components/UsagePanel"
