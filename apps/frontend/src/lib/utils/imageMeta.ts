/** Fetches an `<img>`'s byte size via a `HEAD` request's `Content-Length` header (rounded to KB)
 * — browsers don't expose it otherwise. Resolves to `null` if unavailable. */
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

/** Resize-optimization metadata `serve_page` stamps on a converted page's response, letting the
 * reader's file-info bar show "served WebP" vs "original entry". `null` if unconverted or the HEAD fails. */
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
