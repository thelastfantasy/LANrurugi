import type { Titles } from "@/api/types"

/** The title to show, chosen by the viewer's own ordered language preference.
 *
 * The same list that picks the interface language picks the title, so the two agree: someone whose
 * order is `zh, ja, en` reads a Chinese title where one exists, a Japanese one otherwise, and the
 * romanised one only as a last resort.
 *
 * Deliberately no language is special-cased here. An earlier version preferred `ja` outright, which
 * happened to suit one source and would have been wrong for the next — the host should not hold an
 * opinion about which language is the "real" one. */
export function preferredTitle(
  // Optional on the wire: a record written before titles became a map has no such field, and a
  // required type here would only make the compiler vouch for what the server does not promise.
  titles: Titles | undefined,
  /** Ordered language codes, most preferred first. */
  order: readonly string[],
): string | undefined {
  if (!titles) return undefined
  for (const code of order) {
    const match = titles[code.split("-")[0].toLowerCase()]
    if (match) return match
  }
  // `origin` is how the source itself writes it — the right fallback when none of the preferences
  // are on offer, since it is the one title guaranteed to exist.
  return titles.origin ?? Object.values(titles)[0]
}

/** A source URL as the browser can follow it.
 *
 * Sources are stored normalised — scheme and `www.` stripped — so that the same work under different
 * URL forms compares equal. Handed straight to an `href` that reads as a *relative* path, which sent
 * `e-hentai.org/g/123/abc` to `localhost:3000/e-hentai.org/g/123/abc`. */
export function sourceHref(source: string): string {
  return /^https?:\/\//i.test(source) ? source : `https://${source}`
}

/** Where a candidate's title should lead.
 *
 * When the library already holds the work, the useful destination is the copy that is *here*, not
 * the listing it was discovered on — so `already_held` links to its own archive and `superseded` to
 * the newer revision the library kept. Everything else still points at the source. */
export function titleHref(record: {
  source_url: string
  verdict: { verdict: string; archive_id?: string; newer_archive_id?: string }
}): string {
  const v = record.verdict
  if (v.verdict === "already_held" && v.archive_id) return `/reader/${encodeURIComponent(v.archive_id)}`
  if (v.verdict === "superseded" && v.newer_archive_id)
    return `/reader/${encodeURIComponent(v.newer_archive_id)}`
  return sourceHref(record.source_url)
}
