// Wires the client-composited result into the IndexedDB cache (T051).
//
// Thin by design: `cache.ts` owns the storage mechanics and `composite.ts` owns the drawing, so
// this is only the "check first, composite on a miss, store the result" policy that joins them —
// kept in its own file rather than inlined into either, since both of those are independently
// useful (the cache is also cleared on settings changes; compositing is also used for a re-render
// with no cache involved).

import { type ClientCacheKey,getCachedPage, invalidatePage, putCachedPage } from "./cache"
import { compositePage } from "./composite"
import type { DetectedTextRegion } from "./types"

/**
 * Returns the composited page, from cache when possible.
 *
 * A cache miss is never an error: the entry is re-derivable from the regions, which are the
 * authoritative record (research.md §16).
 */
export async function getOrCompositePage(
  key: ClientCacheKey,
  loadPageImage: () => Promise<ImageBitmap | HTMLImageElement>,
  regions: DetectedTextRegion[],
): Promise<Blob> {
  const cached = await getCachedPage(key)
  if (cached) return cached

  const pageImage = await loadPageImage()
  const blob = await compositePage(pageImage, regions)

  // Storing is best-effort — a full or unavailable quota costs a cache hit, not the page.
  await putCachedPage(key, blob)
  return blob
}

/** Drops a page's cached renderings, e.g. after its translation was corrected. */
export async function invalidateCachedPage(archiveId: string, page: number): Promise<void> {
  await invalidatePage(archiveId, page)
}
