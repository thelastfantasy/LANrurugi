import { withImageCapabilities } from "./imageFormatSupport"
import { isTankoubonId } from "./isTankoubonId"

/**
 * The cover of a search result. With its `cover_version` the URL names exactly one cover file, so
 * the server lets the browser keep it for good and a revisit draws every card from cache; a new
 * cover is a new version, hence a new URL. The codec list is pinned too: the same URL must not
 * hand JXL to a tab that has since found it cannot decode JXL.
 */
export function coverUrl(entry: { arcid: string; cover_version?: string | null }): string {
  const base = isTankoubonId(entry.arcid)
    ? `/api/tankoubons/${entry.arcid}/thumbnail?no_fallback=true`
    : `/api/archives/${entry.arcid}/thumbnail?no_fallback=true`
  if (!entry.cover_version) return base
  return withImageCapabilities(`${base}&cv=${encodeURIComponent(entry.cover_version)}`)
}
