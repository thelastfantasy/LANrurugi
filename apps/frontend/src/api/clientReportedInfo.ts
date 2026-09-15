/** Browser/environment facts only the client itself can know — no HTTP header carries any of
 * these, so the backend can't derive them the way it derives `User-Agent`/`client_ip`. Field
 * names match `lanrurugi-api::login::ClientReportedFields` (Rust, `snake_case`) exactly — these
 * go straight into a form body the backend deserializes into that struct.
 *
 * Every accessor is individually wrapped in try/catch: a privacy-hardened browser (Firefox
 * resist-fingerprinting mode, a locked-down WebView, an old browser missing a newer API like
 * `navigator.deviceMemory`) may throw or simply not expose any one of these, and one missing
 * field must never prevent the rest from being collected.
 *
 * `async` — `navigator.userAgentData.getHighEntropyValues` (Client Hints, the source for the
 * `uach_*` fields) and the incognito-mode heuristic (`navigator.storage.estimate`) are themselves
 * Promise-based; every synchronous field is collected first so a Client-Hints-less browser
 * (Firefox/Safari) still returns everything it can without waiting on an API it doesn't have. */
export async function collectClientReportedInfo(): Promise<Record<string, string>> {
  const fields: Record<string, string> = {}

  const set = (key: string, get: () => string | number | boolean | undefined | null) => {
    try {
      const value = get()
      if (value !== undefined && value !== null && value !== "") fields[key] = String(value)
    } catch {
      // This one field is unavailable in this browser/privacy mode — every other field still
      // gets collected independently.
    }
  }

  set("screen_width", () => window.screen?.width)
  set("screen_height", () => window.screen?.height)
  set("screen_avail_width", () => window.screen?.availWidth)
  set("screen_avail_height", () => window.screen?.availHeight)
  set("window_outer_width", () => window.outerWidth)
  set("window_outer_height", () => window.outerHeight)
  set("window_inner_width", () => window.innerWidth)
  set("window_inner_height", () => window.innerHeight)
  set("device_pixel_ratio", () => window.devicePixelRatio)
  set("color_depth", () => window.screen?.colorDepth)
  set("pixel_depth", () => window.screen?.pixelDepth)
  set("screen_orientation", () => window.screen?.orientation?.type)
  set("language", () => navigator.language)
  set("languages", () => navigator.languages?.join(", "))
  set("timezone", () => Intl.DateTimeFormat().resolvedOptions().timeZone)
  set("timezone_offset_minutes", () => -new Date().getTimezoneOffset())
  set("platform", () => navigator.platform)
  set("hardware_concurrency", () => navigator.hardwareConcurrency)
  // `navigator.deviceMemory` — Chromium-only Device Memory API, no TS lib.dom type for it.
  set("device_memory_gib", () => (navigator as unknown as { deviceMemory?: number }).deviceMemory)
  set("touch_support", () => "ontouchstart" in window || (navigator.maxTouchPoints ?? 0) > 0)
  set("max_touch_points", () => navigator.maxTouchPoints)
  // `navigator.connection` — Chromium-only Network Information API, no TS lib.dom type for it.
  const connection = (
    navigator as unknown as {
      connection?: { effectiveType?: string; downlink?: number; rtt?: number; saveData?: boolean }
    }
  ).connection
  set("connection_type", () => connection?.effectiveType)
  set("connection_downlink_mbps", () => connection?.downlink)
  set("connection_rtt_ms", () => connection?.rtt)
  set("connection_save_data", () => connection?.saveData)
  set("cookie_enabled", () => navigator.cookieEnabled)
  set("pdf_viewer_enabled", () => (navigator as unknown as { pdfViewerEnabled?: boolean }).pdfViewerEnabled)
  set("prefers_dark_color_scheme", () => window.matchMedia?.("(prefers-color-scheme: dark)").matches)
  set("prefers_reduced_motion", () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches)

  // User-Agent Client Hints (`navigator.userAgentData`) — Chromium-only, entirely absent on
  // Firefox/Safari (the `in navigator` check below is what makes that a silent no-op there rather
  // than a thrown error). `getHighEntropyValues` is the only way to get `platformVersion` (the
  // real OS build number — Chrome's plain `User-Agent` string has frozen/generic OS version info
  // for privacy since ~2023) and each brand's *full* version (`brands`/`fullVersionList` only
  // carry major versions until high-entropy values are explicitly requested — a deliberate
  // Client Hints privacy gate, not an oversight here).
  try {
    const uaData = (
      navigator as unknown as {
        userAgentData?: {
          platform?: string
          mobile?: boolean
          brands?: { brand: string; version: string }[]
          getHighEntropyValues?: (
            hints: string[],
          ) => Promise<{
            platformVersion?: string
            fullVersionList?: { brand: string; version: string }[]
          }>
        }
      }
    ).userAgentData
    if (uaData) {
      set("uach_platform", () => uaData.platform)
      set("uach_mobile", () => uaData.mobile)
      set("uach_brands", () => uaData.brands?.map((b) => `${b.brand} ${b.version}`).join(", "))
      if (uaData.getHighEntropyValues) {
        const highEntropy = await uaData.getHighEntropyValues(["platformVersion", "fullVersionList"])
        set("uach_platform_version", () => highEntropy.platformVersion)
        set(
          "uach_full_version_list",
          () => highEntropy.fullVersionList?.map((b) => `${b.brand} ${b.version}`).join(", "),
        )
      }
    }
  } catch {
    // Client Hints entirely unavailable (Firefox/Safari) or the high-entropy request itself
    // failed/was denied — every other field above is already collected regardless.
  }

  // Heuristic-only private/incognito-browsing guess — no browser exposes a real "am I in private
  // mode" flag (deliberately, as part of that mode's own privacy design). Chrome's incognito mode
  // caps `navigator.storage.estimate()`'s reported quota far below a normal session's (historically
  // ~120MB vs. a large fraction of free disk space normally) — crossing a fixed, generously-below-
  // any-real-normal-quota threshold is treated as "probably incognito", never a certainty. `None`
  // (this field simply absent) on any browser where the API itself is unavailable or the check
  // throws — Firefox/Safari don't expose the same signal, so absence here must not be read as
  // "normal browsing" on those.
  const INCOGNITO_QUOTA_THRESHOLD_BYTES = 200 * 1024 * 1024
  try {
    const estimate = await navigator.storage?.estimate?.()
    if (estimate?.quota !== undefined) {
      fields.probably_incognito = String(estimate.quota < INCOGNITO_QUOTA_THRESHOLD_BYTES)
    }
  } catch {
    // `navigator.storage.estimate()` unavailable/denied — leave `probably_incognito` unset.
  }

  return fields
}
