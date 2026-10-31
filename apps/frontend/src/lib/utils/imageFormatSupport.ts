/**
 * One-time browser image-format feature detection, parked in a cookie so the Rust backend can
 * choose the best representation per request without every `<img>` URL needing an extra query
 * parameter.
 *
 * Two things travel in that signal. The *preference* chain (JXL > WebP > original source) decides
 * which codec the reader asks for, and the *capability set* tells the backend which source formats
 * this browser cannot render at all — a page in one of those has to be converted rather than passed
 * through, whatever the size threshold says. AVIF is the case that matters today: Firefox only
 * gained still-AVIF support in 93 (sequences behind a pref) and Safari needs 16.4, so an AVIF page
 * served as-is is simply a broken image there.
 */

export type ImageFormatSupport = "jxl" | "webp" | "source"

/** Decode capabilities, mirrored by `ClientImageSupport` in `crates/lanrurugi-api/src/archives.rs`. */
export interface ImageDecodeCapabilities {
  jxl: boolean
  webp: boolean
  avif: boolean
  /**
   * AVIF *sequences* are their own capability: Firefox 93-112 and Safari/iOS 16.0-16.3 decode
   * still AVIF but not animated AVIF (Firefox 111-112 needs `image.avif.sequence.enabled`), so a
   * still-image probe cannot answer for an animated page. No such split exists for WebP/GIF/PNG —
   * a client that decodes those still decodes their animations — which is why only AVIF gets an
   * extra flag.
   */
  avifAnim: boolean
  /**
   * The mirror case of `avifAnim`: Safari (up to at least 27.x) and older Chrome/Firefox decode
   * still JXL but not JXL *sequences* — caniuse's Safari footnote is literally "Supports still
   * images. Animated image sequences are not supported." Chrome 155+ and Firefox 158+ play both.
   */
  jxlAnim: boolean
}

/** Keep in sync with `crates/lanrurugi-api/src/archives.rs`'s cookie/header reader. */
export const IMAGE_FORMAT_SUPPORT_COOKIE = "lrr_img_support"

export const IMAGE_FORMAT_SUPPORT_HEADER = "X-LRR-Image-Support"

/** Tiny valid WebP image, used only for asynchronous image-decoder detection. */
const TINY_WEBP_BASE64 = "UklGRhwAAABXRUJQVlA4TA8AAAAvAAAAAAcQ/Y/+ByKi/wEA"

/** Tiny valid JXL image, used only for asynchronous image-decoder detection. */
const TINY_JXL_BASE64 =
  "/woIAAKASAgCAQDMAksYm5xxhAM4gAM4IErAOQUBACBEgAgQASJAhP/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uhv/37/nvoTHnnGvtc2+SJAkBVVVVVVXV////c+/r7u7uPgDHvwA8AB7+j/7n/of/1X/y49EP"

/**
 * Five-frame animated JXL (256x256), for animation detection. Byte-identical to
 * `crates/lanrurugi-scanner/tests/fixtures/animated_5_frames.jxl` — the sample shipped in the
 * `jxl` crate's own test data (BSD-3-Clause, synthetic "numbered frames" content).
 */
const TINY_ANIMATED_JXL_BASE64 =
  "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"

/**
 * Tiny animated AVIF (two 8x8 frames, `ftyp` brand `avis`), for animation detection.
 *
 * UNVERIFIED sample: this Chrome 154 reports `avifAnim: false` for it while reporting `jxlAnim:
 * true` through the same WebCodecs path (so the probe itself works). It may be that ffmpeg's AVIF
 * muxer wrote a sequence Chromium will not play, or that this build genuinely lacks AVIF-sequence
 * playback. The flag is currently behaviour-neutral — no AVIF decoder exists on the server, so an
 * AVIF page is served as-is either way — but it must be re-verified against a known-good sample
 * (e.g. https://colinbendell.github.io/webperf/animated-gif-decode/3.avif) before it can gate a
 * real conversion. A false value is the safe direction: it means "convert", not "pass through".
 */
const TINY_ANIMATED_AVIF_BASE64 =
  "AAAALGZ0eXBhdmlzAAAAAGF2aXNhdmlmbXNmMWlzbzhtaWYxbWlhZk1BMUIAAAD5bWV0YQAAAAAAAAAvaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAFBpY3R1cmVIYW5kbGVyAAAAAA5waXRtAAAAAAABAAAAHmlsb2MAAAAARAAAAQABAAAAAQAAA+gAAAAgAAAAKGlpbmYAAAAAAAEAAAAaaW5mZQIAAAAAAQAAYXYwMUNvbG9yAAAAAGppcHJwAAAAS2lwY28AAAAUaXNwZQAAAAAAAAAIAAAACAAAABBwaXhpAAAAAAMICAgAAAAMYXYxQ4EADAAAAAATY29scm5jbHgAAgACAAIAAAAAF2lwbWEAAAAAAAAAAQABBAECgwQAAAK7bW9vdgAAAGxtdmhkAAAAAAAAAAAAAAAAAAAD6AAAA+gAAQAAAQAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgAAAkd0cmFrAAAAaHRraGQBAAADAAAAAAAAAAAAAAAAAAAAAAAAAAEAAAAAf/////////8AAAAAAAAAAAAAAAAAAAAAAAEAAAAAAAAAAAAAAAAAAAABAAAAAAAAAAAAAAAAAABAAAAAAAgAAAAIAAAAAAAkZWR0cwAAABxlbHN0AAAAAQAAAAEAAAPoAAAAAAABAAAAAAGzbWRpYQAAACBtZGhkAAAAAAAAAAAAAAAAAABAAAAAQABVxAAAAAAAL2hkbHIAAAAAAAAAAHBpY3QAAAAAAAAAAAAAAABQaWN0dXJlSGFuZGxlcgAAAAFcbWluZgAAABR2bWhkAAAAAQAAAAAAAAAAAAAAJGRpbmYAAAAcZHJlZgAAAAAAAAABAAAADHVybCAAAAABAAABHHN0YmwAAACcc3RzZAAAAAAAAAABAAAAjGF2MDEAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAACAAIAEgAAABIAAAAAAAAAAEYTGF2YzYyLjExLjEwMCBsaWJhb20tYXYxAAAAAAAAAAAY//8AAAAMYXYxQ4EADAAAAAAKZmllbAEAAAAAEHBhc3AAAAABAAAAAQAAABBjY3N0AAAAAHwAAAAAAAAYc3R0cwAAAAAAAAABAAAAAgAAIAAAAAAUc3RzcwAAAAAAAAABAAAAAQAAABxzdHNjAAAAAAAAAAEAAAABAAAAAgAAAAEAAAAcc3RzegAAAAAAAAAAAAAAAgAAACAAAAAjAAAAFHN0Y28AAAAAAAAAAQAAA+gAAABLbWRhdAoJAAAAARfn/8wCMhMQALcAAAJAAAIDVJYBwPNkSDMoMiEwA8CAAAAGwAAAAoAGAoAAoN2TlcMY6b8/dGHqKyypV+E="

/** Tiny valid AVIF (2x2), used only for asynchronous image-decoder detection. */
const TINY_AVIF_BASE64 =
  "AAAAIGZ0eXBhdmlmAAAAAGF2aWZtaWYxbWlhZk1BMUIAAAD5bWV0YQAAAAAAAAAvaGRscgAAAAAAAAAAcGljdAAAAAAAAAAAAAAAAFBpY3R1cmVIYW5kbGVyAAAAAA5waXRtAAAAAAABAAAAHmlsb2MAAAAARAAAAQABAAAAAQAAASEAAAAgAAAAKGlpbmYAAAAAAAEAAAAaaW5mZQIAAAAAAQAAYXYwMUNvbG9yAAAAAGppcHJwAAAAS2lwY28AAAAUaXNwZQAAAAAAAAACAAAAAgAAABBwaXhpAAAAAAMICAgAAAAMYXYxQ4EADAAAAAATY29scm5jbHgAAgACAAIAAAAAF2lwbWEAAAAAAAAAAQABBAECgwQAAAAobWRhdAoJAAAAAAZ//MAgMhMQAMAAAAKAAAIDUu3nF/TzRaYg"

let cachedSupport: ImageFormatSupport | null = null
let cachedCapabilities: ImageDecodeCapabilities | null = null

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
  type: "image/jxl" | "image/webp" | "image/avif",
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

/**
 * Whether the browser actually *animates* a sample of `type`. WebCodecs' `ImageDecoder` reports a
 * real frame count, which is the only cheap signal that separates "decodes still AVIF" from
 * "plays AVIF sequences"; where it is unavailable, render the sample twice and see whether the
 * pixels changed.
 */
async function detectAnimationSupport(
  type: "image/webp" | "image/avif" | "image/jxl",
  base64: string,
): Promise<boolean> {
  // `ImageDecoder`'s `data` accepts an ArrayBuffer/view or a ReadableStream — *not* a `Blob`
  // (Chromium throws `TypeError` for one), so the bytes are passed directly.
  const bytes = Uint8Array.from(atob(base64), (char) => char.charCodeAt(0))
  const blob = base64ToBlob(base64, type)
  // Cast through `unknown`: the DOM lib's own `ImageDecoder`/`ImageDecoderInit` types are stricter
  // than the minimal shape used here, and this must also compile where they are absent.
  interface MinimalImageDecoder {
    tracks: {
      ready: Promise<unknown>
      length: number
      [index: number]: { frameCount?: number; animated?: boolean } | undefined
    }
    close?: () => void
  }
  const decoderCtor = (
    globalThis as unknown as {
      ImageDecoder?: new (init: unknown) => MinimalImageDecoder
    }
  ).ImageDecoder
  if (decoderCtor) {
    try {
      const decoder = new decoderCtor({ data: bytes, type })
      const tracks = decoder.tracks
      // A first decode can take a moment even for a tiny sample; the earlier 800ms budget was what
      // made this path silently give up.
      const ready = await withTimeout(tracks.ready.then(() => tracks), 3000)
      if (ready && ready.length > 0) {
        const track = ready[0]
        const frames = track?.frameCount ?? 0
        decoder.close?.()
        return frames > 1 || track?.animated === true
      }
      decoder.close?.()
    } catch {
      // Fall through to the pixel-sampling detector.
    }
  }

  return new Promise((resolve) => {
    const img = new Image()
    const url = URL.createObjectURL(blob)
    const canvas = document.createElement("canvas")
    canvas.width = 8
    canvas.height = 8
    const context = canvas.getContext("2d", { willReadFrequently: true })
    if (!context) {
      URL.revokeObjectURL(url)
      resolve(false)
      return
    }
    const sample = () => {
      context.drawImage(img, 0, 0, 8, 8)
      return context.getImageData(0, 0, 8, 8).data.join(",")
    }
    const finish = (value: boolean) => {
      window.clearTimeout(timer)
      URL.revokeObjectURL(url)
      img.remove()
      resolve(value)
    }
    const timer = window.setTimeout(() => finish(false), 4000)
    img.onload = () => {
      // Attached, off-screen: a detached image is not animated by Chromium, which would make this
      // detector answer "no animation" for every sample.
      img.style.position = "fixed"
      img.style.left = "-9999px"
      img.style.width = "8px"
      img.style.height = "8px"
      document.body.appendChild(img)
      const first = sample()
      window.setTimeout(() => finish(sample() !== first), 500)
    }
    img.onerror = () => finish(false)
    img.src = url
  })
}

function readCookieValue(): string | null {
  const match = document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith(`${IMAGE_FORMAT_SUPPORT_COOKIE}=`))
  return match ? match.slice(IMAGE_FORMAT_SUPPORT_COOKIE.length + 1) : null
}

/** Parses the current list form (`jxl,webp,avif`) and the legacy single values. */
function parseCapabilities(value: string): ImageDecodeCapabilities | null {
  const trimmed = value.trim().toLowerCase()
  if (!trimmed) return null
  if (trimmed.includes(",")) {
    const caps: ImageDecodeCapabilities = {
      jxl: false,
      webp: false,
      avif: false,
      avifAnim: false,
      jxlAnim: false,
    }
    for (const part of trimmed.split(",").map((p) => p.trim())) {
      if (part === "jxl") caps.jxl = true
      else if (part === "webp") caps.webp = true
      else if (part === "avif") caps.avif = true
      // Sequence support implies still support.
      else if (part === "avif-anim") {
        caps.avif = true
        caps.avifAnim = true
      } else if (part === "jxl-anim") {
        caps.jxl = true
        caps.jxlAnim = true
      } else if (part !== "" && part !== "source" && part !== "original" && part !== "none") return null
    }
    return caps
  }
  switch (trimmed) {
    case "jxl":
      return { jxl: true, webp: true, avif: false, avifAnim: false, jxlAnim: false }
    case "webp":
      return { jxl: false, webp: true, avif: false, avifAnim: false, jxlAnim: false }
    case "source":
    case "original":
    case "none":
      return { jxl: false, webp: false, avif: false, avifAnim: false, jxlAnim: false }
    default:
      return null
  }
}

function readCapabilitiesCookie(): ImageDecodeCapabilities | null {
  const value = readCookieValue()
  return value ? parseCapabilities(value) : null
}

/** Wire form of the capability set: the comma list the backend's `ClientImageSupport` parses. */
export function capabilitiesToWire(caps: ImageDecodeCapabilities): string {
  const parts: string[] = []
  if (caps.jxl) parts.push("jxl")
  if (caps.webp) parts.push("webp")
  if (caps.avif) parts.push("avif")
  if (caps.avifAnim) parts.push("avif-anim")
  if (caps.jxlAnim) parts.push("jxl-anim")
  return parts.length > 0 ? parts.join(",") : "source"
}

function supportToPreference(caps: ImageDecodeCapabilities): ImageFormatSupport {
  return caps.jxl ? "jxl" : caps.webp ? "webp" : "source"
}

/** Runs once from `main.tsx` before React mounts, then caches the result for request headers. */
export async function installImageFormatSupportCookie(): Promise<ImageFormatSupport> {
  const [jxl, webp, avif] = await Promise.all([
    detectDecodeSupport("image/jxl", TINY_JXL_BASE64),
    detectDecodeSupport("image/webp", TINY_WEBP_BASE64),
    detectDecodeSupport("image/avif", TINY_AVIF_BASE64),
  ])
  // Only probe AVIF sequences, and only where still AVIF already decodes: no browser is known to
  // split still/animated for WebP, and probing a codec the browser lacks is a guaranteed "no".
  const [avifAnim, jxlAnim] = await Promise.all([
    avif ? detectAnimationSupport("image/avif", TINY_ANIMATED_AVIF_BASE64) : Promise.resolve(false),
    jxl ? detectAnimationSupport("image/jxl", TINY_ANIMATED_JXL_BASE64) : Promise.resolve(false),
  ])
  cachedCapabilities = { jxl, webp, avif, avifAnim, jxlAnim }
  const support = supportToPreference(cachedCapabilities)
  cachedSupport = support
  document.cookie = `${IMAGE_FORMAT_SUPPORT_COOKIE}=${capabilitiesToWire(
    cachedCapabilities,
  )}; Path=/; Max-Age=31536000; SameSite=Lax`
  return support
}

export function getImageDecodeCapabilities(): ImageDecodeCapabilities {
  return (
    cachedCapabilities ??
    readCapabilitiesCookie() ?? {
      jxl: false,
      webp: true,
      avif: false,
      avifAnim: false,
      jxlAnim: false,
    }
  )
}

/** For `fetch()` callers (the reader) that want an explicit header rather than relying on cookies. */
export function getImageFormatSupport(): ImageFormatSupport {
  return cachedSupport ?? supportToPreference(getImageDecodeCapabilities())
}

/** Capability list for the backend's necessity check (a client that cannot render the source). */
export function getImageCapabilityHeader(): string {
  return capabilitiesToWire(getImageDecodeCapabilities())
}
