import { getImageCapabilityHeader,IMAGE_FORMAT_SUPPORT_HEADER } from "@/lib/utils/imageFormatSupport"

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
 * reader's file-info bar show "served WebP/JXL" vs "original entry". `null` if unconverted or the
 * HEAD fails. */
export interface ResizedPageInfo {
  servedFormat: "webp" | "jxl"
  origSizeBytes: number
  origWidth: number
  origHeight: number
}

export async function fetchResizedPageInfo(url: string): Promise<ResizedPageInfo | null> {
  try {
    const res = await fetch(url, { method: "HEAD" })
    const servedFormat = res.headers.get("x-lrr-resized")
    if (servedFormat !== "webp" && servedFormat !== "jxl") return null
    const origSize = Number(res.headers.get("x-lrr-original-size"))
    const dims = res.headers.get("x-lrr-original-dimensions")?.split("x")
    if (Number.isNaN(origSize) || !dims) return null
    const origWidth = Number(dims[0])
    const origHeight = Number(dims[1])
    if (Number.isNaN(origWidth) || Number.isNaN(origHeight)) return null
    return { servedFormat, origSizeBytes: origSize, origWidth, origHeight }
  } catch {
    return null
  }
}

/** What a metadata-only HEAD tells the reader about one page: how big the entry is, what box it
 * occupies, and whether the server will hand back those bytes as-is or has to convert them. The
 * reader only needs its preview placeholder in the converting case — a passthrough page paints from
 * its own first frame long before a placeholder could arrive. */
export interface PageMeta {
  sizeKb: number | null
  width: number
  height: number
  servedOriginal: boolean | null
}

export async function fetchPageMeta(url: string): Promise<PageMeta | null> {
  try {
    const res = await fetch(url, {
      method: "HEAD",
      headers: { [IMAGE_FORMAT_SUPPORT_HEADER]: getImageCapabilityHeader() },
    })
    if (!res.ok) return null
    const bytes = Number(res.headers.get("Content-Length"))
    const dims = res.headers.get("x-lrr-page-dimensions")
    const [width, height] = (dims ?? "").split("x").map((v) => Number(v))
    const served = res.headers.get("x-lrr-served-original")
    return {
      sizeKb: Number.isNaN(bytes) ? null : Math.floor(bytes / 1024),
      width: Number.isFinite(width) && width > 0 ? width : 0,
      height: Number.isFinite(height) && height > 0 ? height : 0,
      servedOriginal: served === "1" ? true : served === "0" ? false : null,
    }
  } catch {
    return null
  }
}
