/** Fetches an already-loaded `<img>`'s own byte size via a `HEAD` request's `Content-Length`
 * header, rounded down to whole KB — browsers don't expose response size for a plain `<img>` load
 * itself, so this is a second, deliberate request against the same URL (already in the browser's
 * HTTP cache from the `<img>` load, so it's cheap) purely to read that header. Resolves to `null`
 * if the header is missing/unparseable or the request itself fails (e.g. offline), matching the
 * pre-extraction callers' own silent-skip behavior — the caller's own dimensions/other metadata
 * already rendered by this point and shouldn't block or error out on this being unavailable. */
export async function fetchContentLengthKb(url: string): Promise<number | null> {
  try {
    const res = await fetch(url, { method: 'HEAD' })
    const bytes = Number(res.headers.get('Content-Length'))
    if (Number.isNaN(bytes)) return null
    return Math.floor(bytes / 1024)
  } catch {
    return null
  }
}
