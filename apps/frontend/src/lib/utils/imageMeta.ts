/** Fetches an already-loaded `<img>`'s own byte size via a `HEAD` request's `Content-Length`
 * header, rounded down to whole KB — browsers don't expose response size for a plain `<img>` load
 * itself, so this is a second, deliberate request against the same URL (already in the browser's
 * HTTP cache from the `<img>` load, so it's cheap) purely to read that header. Resolves to `null`
 * if the header is missing/unparseable or the request itself fails (e.g. offline), matching the
 * pre-extraction callers' own silent-skip behavior — the caller's own dimensions/other metadata
 * already rendered by this point and shouldn't block or error out on this being unavailable. */
export async function fetchContentLengthKb(url: string): Promise<number | null> {
  try {
    const res = await fetch(url, { method: "HEAD" })
    const bytes = Number(res.headers.get("Content-Length"))
    if (Number.isNaN(bytes)) return null
    return Math.floor(bytes / 1024)
  } catch {
    return null
  }
}

/** The resize-optimization metadata `serve_page` stamps on a converted page's response — lets
 * the reader's file-info bar show "served WebP" vs "original entry" side by side. `null` when the
 * page wasn't converted (no `x-lrr-resized` header) or the `HEAD` itself fails. */
export interface ResizedPageInfo {
  origSizeBytes: number
  origWidth: number
  origHeight: number
}

export async function fetchResizedPageInfo(url: string): Promise<ResizedPageInfo | null> {
  try {
    const res = await fetch(url, { method: "HEAD" })
    if (res.headers.get("x-lrr-resized") !== "webp") return null
    const origSize = Number(res.headers.get("x-lrr-original-size"))
    const dims = res.headers.get("x-lrr-original-dimensions")?.split("x")
    if (Number.isNaN(origSize) || !dims) return null
    const origWidth = Number(dims[0])
    const origHeight = Number(dims[1])
    if (Number.isNaN(origWidth) || Number.isNaN(origHeight)) return null
    return { origSizeBytes: origSize, origWidth, origHeight }
  } catch {
    return null
  }
}
