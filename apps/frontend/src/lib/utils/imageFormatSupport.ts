/**
 * One-time browser image-format feature detection, parked in a cookie so the Rust backend can
 * choose the best representation per request without every `<img>` URL needing an extra query
 * parameter.
 *
 * Preference is JXL > WebP > original source. Only JXL *decode* support matters here; the server
 * serves a JXL-native page as-is when we advertise JXL, and otherwise decodes it and re-encodes
 * to WebP (or serves the source bytes to a browser that cannot decode WebP either).
 */

export type ImageFormatSupport = "jxl" | "webp" | "source"

/** Keep in sync with `crates/lanrurugi-api/src/archives.rs`'s cookie/header reader. */
export const IMAGE_FORMAT_SUPPORT_COOKIE = "lrr_img_support"

export const IMAGE_FORMAT_SUPPORT_HEADER = "X-LRR-Image-Support"

/** Tiny valid WebP image, used only for asynchronous image-decoder detection. */
const TINY_WEBP_BASE64 = "UklGRhwAAABXRUJQVlA4TA8AAAAvAAAAAAcQ/Y/+ByKi/wEA"

/** Tiny valid JXL image, used only for asynchronous image-decoder detection. */
const TINY_JXL_BASE64 =
  "/woIAAKASAgCAQDMAksYm5xxhAM4gAM4IErAOQUBACBEgAgQASJAhP/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uPgDHvwA8AB7+j/7n/of/1X/y49EP"

let cachedSupport: ImageFormatSupport | null = null

function base64ToBlob(base64: string, type: string): Blob {
  const binary = atob(base64)
  const bytes = Uint8Array.from(binary, (char) => char.charCodeAt(0))
  return new Blob([bytes], { type })
}

function detectViaImageElement(dataUrl: string): Promise<boolean> {
  return new Promise((resolve) => {
    const image = new Image()
    const timer = window.setTimeout(() => resolve(false), 800)
    image.onload = () => {
      window.clearTimeout(timer)
      resolve(true)
    }
    image.onerror = () => {
      window.clearTimeout(timer)
      resolve(false)
    }
    image.src = dataUrl
  })
}

function withTimeout<T>(promise: Promise<T>, timeoutMs: number): Promise<T | null> {
  return new Promise((resolve) => {
    const timer = window.setTimeout(() => resolve(null), timeoutMs)
    promise.then(
      (value) => {
        window.clearTimeout(timer)
        resolve(value)
      },
      () => {
        window.clearTimeout(timer)
        resolve(null)
      },
    )
  })
}

async function detectDecodeSupport(
  type: "image/jxl" | "image/webp",
  base64: string,
): Promise<boolean> {
  // WebCodecs' `ImageDecoder.isTypeSupported` is the cheapest and least side-effectful signal on
  // current Chromium. Fall back to actually decoding a tiny embedded sample where unavailable.
  const decoderCtor = (globalThis as { ImageDecoder?: { isTypeSupported?: (type: string) => Promise<boolean> } }).ImageDecoder
  if (decoderCtor?.isTypeSupported) {
    try {
      const supported = await withTimeout(decoderCtor.isTypeSupported(type), 800)
      if (supported !== null) return supported
    } catch {
      // Fall through to the bitmap detector.
    }
  }

  if (typeof createImageBitmap === "function") {
    try {
      const bitmap = await withTimeout(createImageBitmap(base64ToBlob(base64, type)), 800)
      if (bitmap) {
        bitmap.close?.()
        return true
      }
    } catch {
      // A browser can decode WebP through `<img>` even where `createImageBitmap` is unavailable;
      // try the image-element path below before giving up.
    }
  }

  return detectViaImageElement(`data:${type};base64,${base64}`)
}

function readSupportCookie(): ImageFormatSupport | null {
  const match = document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${IMAGE_FORMAT_SUPPORT_COOKIE}=`))
  if (!match) return null
  const value = match.slice(IMAGE_FORMAT_SUPPORT_COOKIE.length + 1)
  return value === "jxl" || value === "webp" || value === "source" ? value : null
}

/** Runs once from `main.tsx` before React mounts, then caches the result for request headers. */
export async function installImageFormatSupportCookie(): Promise<ImageFormatSupport> {
  const [jxl, webp] = await Promise.all([
    detectDecodeSupport("image/jxl", TINY_JXL_BASE64),
    detectDecodeSupport("image/webp", TINY_WEBP_BASE64),
  ])
  const support: ImageFormatSupport = jxl ? "jxl" : webp ? "webp" : "source"
  cachedSupport = support
  // `jxl` implies a modern decode stack that also handles WebP, matching the Rust reader's
  // `ClientImageSupport::from_value` handling.
  document.cookie = `${IMAGE_FORMAT_SUPPORT_COOKIE}=${support}; Path=/; Max-Age=31536000; SameSite=Lax`
  return support
}

/** For `fetch()` callers (the reader) that want an explicit header rather than relying on cookies. */
export function getImageFormatSupport(): ImageFormatSupport {
  return cachedSupport ?? readSupportCookie() ?? "webp"
}
