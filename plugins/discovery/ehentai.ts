// E-Hentai discovery: what is new that matches a subscription's criteria (issue #55).
//
// The first worked example of the `discover` capability. Everything site-specific lives here; the
// host does the scheduling, filtering, and deduplication.
//
// Read `degraded` below before changing anything in this file. E-Hentai returns a *different,
// smaller* result set when signed out rather than an error — the request succeeds and simply shows
// less. Failing to report that would let the host record works it never saw as handled.

export function pluginInfo() {
  return {
    namespace: "ehdiscover",
    type: "discovery" as const,
    parameters: [],
    declared_permissions: {
      // The JSON API is where a work's own-language title lives; the listing only ever carries one
      // title, so a Japanese original is unreachable without it.
      net: ["e-hentai.org", "exhentai.org", "api.e-hentai.org"],
      read: false,
      write: false,
    },
    name: "E*Hentai Discovery",
    author: "thelastfantasy",
    description:
      "Finds new galleries matching a subscription's creator/tag criteria, for the subscription orchestrator.",
    version: "0.1",
    login_from: "ehlogin",
    url_pattern: "e-?hentai\\.org|exhentai\\.org",
    domain_match: ["e-hentai.org", "exhentai.org"],
  };
}

export function pluginOptions() {
  return {
    check_interval: {
      // Deliberately conservative. A subscription exists to notice new works within hours, not
      // seconds, and E-Hentai bans aggressive clients — a cost the user has no way to anticipate,
      // which is why the minimum is a floor the host enforces rather than a hint.
      suggested_secs: 6 * 60 * 60,
      minimum_secs: 60 * 60,
      description:
        "How often to check E-Hentai for new matching galleries. Checking too often risks being rate-limited or banned.",
    },
  };
}

/** Same normalisation `plugins/download/ehentai.ts` uses, so a work discovered here compares equal
 * to the same work already held. Divergence between the two would silently defeat the host's
 * duplicate check — it compares strings, and has no way to notice they were produced differently. */
export function canonicalizeSource(url: string): string {
  const generic = String(url ?? "")
    .trim()
    .replace(/^https?:\/\//i, "")
    .replace(/^www\./i, "")
    .split("?")[0]
    .split("#")[0]
    .replace(/\/+$/, "");
  const match = generic.match(
    /^(?:g\.|forums\.)?(?:ex|e-)hentai\.org\/g\/([0-9]+)\/([0-9a-f]+)/i,
  );
  return match ? `e-hentai.org/g/${match[1]}/${match[2]}` : generic;
}

/** Builds the search URL for a subscription's criteria. */
function searchUrl(criteria: DiscoveryCriteriaArgs): string {
  const terms: string[] = [];
  if (criteria.creator) {
    // E-Hentai's own namespaced-tag syntax; quoting keeps multi-word handles intact.
    terms.push(`uploader:"${criteria.creator}"`);
  }
  for (const tag of criteria.tags ?? []) {
    terms.push(`"${tag}"`);
  }
  const query = encodeURIComponent(terms.join(" "));
  return `https://e-hentai.org/?f_search=${query}`;
}

/** True when the response is E-Hentai's signed-out view rather than a real result set.
 *
 * Checked explicitly rather than inferred from an empty result, because "no matches" and "you are
 * not signed in" both produce few or no gallery links — and conflating them is precisely the
 * failure this capability's contract warns about. */
function looksSignedOut(html: string): boolean {
  return (
    html.includes("This page requires you to log on") ||
    html.includes("You must be logged in") ||
    html.includes("/bounce_login.php")
  );
}

/** The cookies and headers the host already ran this source's login plugin for.
 *
 * `discover` gets them on its own criteria object (`with_login_cookies` on the host side), and
 * *not* applying them is what a signed-out listing looks like from here: E-Hentai answers a
 * logged-out search with fewer galleries, and `looksSignedOut` only catches the outright
 * "please log in" pages, not a quietly smaller result set. The download plugin has always applied
 * them (to its `userAgent()` jar); a plain `fetch` has no jar, so it has to set the header. */
function authHeaders(criteria: DiscoveryCriteriaArgs): Record<string, string> {
  const info = criteria as unknown as {
    user_agent_cookies?: { name: string; value: string }[]
    user_agent_headers?: Record<string, string>
  };
  const headers: Record<string, string> = { ...(info.user_agent_headers ?? {}) };
  const cookie = (info.user_agent_cookies ?? [])
    .filter((c) => c?.name)
    .map((c) => `${c.name}=${c.value}`)
    .join("; ");
  if (cookie) headers["Cookie"] = cookie;
  return headers;
}

/** True when Cloudflare answered instead of E-Hentai.
 *
 * E-Hentai sits behind Cloudflare (`server: cloudflare`, verified live), and an interstitial can
 * arrive as HTTP 200 carrying a challenge page rather than a listing. That is the dangerous shape:
 * the request "succeeded", the gallery-link pattern matches nothing, and a cycle that trusted it
 * would conclude "nothing new" and mark the works it never saw as handled — permanently hiding them.
 * Reported as a hard error, not as `degraded`, because nothing on the page is a real answer. */
function looksLikeBotChallenge(html: string): boolean {
  return (
    html.includes("cf-browser-verification") ||
    html.includes("/cdn-cgi/challenge-platform/") ||
    html.includes("cf_chl_opt") ||
    html.includes("challenges.cloudflare.com/turnstile") ||
    html.includes("<title>Just a moment")
  );
}

/** Decodes E-Hentai's star rating, which the listing encodes as a sprite offset rather than a number.
 *
 * `background-position: Xpx Ypx` — X moves left 16px per missing star, and Y of -1 (instead of -21)
 * marks a half star. Verified against a live listing: `0px -21px` is 5, `-32px -21px` is 3,
 * `-64px -1px` is 0.5. */
function parseRating(style: string): number | undefined {
  const x = Number(style.match(/background-position:\s*(-?\d+)px/)?.[1]);
  const y = Number(style.match(/px\s+(-?\d+)px/)?.[1]);
  if (!Number.isFinite(x) || !Number.isFinite(y)) return undefined;
  const rating = 5 - Math.abs(x) / 16 - (y === -1 ? 0.5 : 0);
  return rating >= 0 && rating <= 5 ? rating : undefined;
}

/** Decodes the HTML entities a listing title can contain. No DOM is available in the plugin runtime,
 * and an un-decoded `&amp;` in a title is what the user would then read in the approval list. */
function decodeEntities(text: string): string {
  return text
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#(\d+);/g, (_, d) => String.fromCharCode(Number(d)))
    .replace(/&amp;/g, "&");
}

/** Fills in each candidate's own-language title and authoritative post time from the JSON API.
 *
 * The listing carries exactly one title per work — whichever the uploader chose — and its visible
 * posted-at text has no timezone attached. `gdata` supplies the Japanese title, `posted` as a Unix
 * timestamp (so no dependence on how the listing rendered a local date), and the **uploader** —
 * which the signed-in listing itself does not carry at all (only the guest 25-per-page layout links
 * `uploader/<name>`, and a check runs signed in). Reading it from here is what keeps `uploader`
 * populated for the layout a subscription actually sees, at no extra request. It also carries the
 * gallery's whole size (`filesize`, bytes), which no listing row shows — the one value a "not bigger
 * than N MB" rule needs.
 *
 * Failure is swallowed: a missing original title is worth less than the listing itself, and losing
 * the whole check over it would be the wrong trade. */
async function addOriginalTitles(
  candidates: DiscoveredCandidateResult[],
  criteria: DiscoveryCriteriaArgs,
  logger: { warn: (m: string) => void },
): Promise<void> {
  const pairs: [number, string][] = [];
  for (const c of candidates) {
    const m = c.source.match(/\/g\/(\d+)\/([0-9a-f]{10})/);
    if (m) pairs.push([Number(m[1]), m[2]]);
  }
  if (pairs.length === 0) return;

  // E-Hentai's `gdata` endpoint rejects a `gidlist` longer than 25 with a 200-level
  // `{error: "too many gidlist requests"}` body. A subscription pages through up to four listings
  // (400 candidates on a broad query), so sending them all at once silently lost every original
  // title and left only the English `origin` that the listing itself carried. Batch by the
  // endpoint's own limit instead.
  const GDATA_BATCH_SIZE = 25;
  // Batches in flight at once. Awaiting all sixteen batches of a 400-candidate listing one after
  // another cost ~5.6s of pure round-trip latency (measured: a 7.7s preview), while an unbounded
  // `Promise.all` would fire them all at a site that rate-limits this endpoint — a small fixed
  // window gets most of the saving without inviting that.
  const GDATA_CONCURRENCY = 4;
  const metaByGid = new Map<
    string,
    { jpn?: string; posted?: string; uploader?: string; size?: number }
  >();

  const batches: [number, string][][] = [];
  for (let offset = 0; offset < pairs.length; offset += GDATA_BATCH_SIZE) {
    batches.push(pairs.slice(offset, offset + GDATA_BATCH_SIZE));
  }

  const fetchBatch = async (batch: [number, string][]): Promise<void> => {
    try {
      const response = await fetch("https://api.e-hentai.org/api.php", {
        method: "POST",
        // The same credentials the listing gets: a plain `fetch` has no cookie jar, and this API
        // answers with more (and with anything at all for login-gated galleries) when signed in.
        headers: { "Content-Type": "application/json", ...authHeaders(criteria) },
        // `namespace: 1` keeps tag prefixes intact. This call reads only titles, but the older
        // `namespace: 0` strips them, and copying that choice here would mislead the next reader.
        body: JSON.stringify({ method: "gdata", gidlist: batch, namespace: 1 }),
      });
      if (!response.ok) return;
      const data = await response.json();
      for (const entry of data?.gmetadata ?? []) {
        const gid = String(entry.gid);
        const meta = metaByGid.get(gid) ?? {};
        if (entry?.title_jpn) meta.jpn = entry.title_jpn;
        // Always seconds since the Unix epoch in UTC, irrespective of the listing's own display.
        if (entry?.posted) meta.posted = String(entry.posted);
        if (entry?.uploader) meta.uploader = String(entry.uploader);
        // Whole-gallery size in bytes, as a string on the wire (verified live: 140969895 for a
        // 293-file gallery). The listing rows carry no size at all, so this is the only place it can
        // come from — and it costs nothing extra, since this call runs for the titles regardless.
        if (entry?.filesize) {
          const bytes = Number(entry.filesize);
          if (Number.isFinite(bytes)) meta.size = bytes;
        }
        metaByGid.set(gid, meta);
      }
    } catch (e) {
      // One batch failing should not discard titles already collected from the others.
      logger.warn(`could not read original titles or posted timestamps: ${String(e)}`);
    }
  };

  for (let offset = 0; offset < batches.length; offset += GDATA_CONCURRENCY) {
    await Promise.all(batches.slice(offset, offset + GDATA_CONCURRENCY).map(fetchBatch));
  }

  for (const c of candidates) {
    const gid = c.source.match(/\/g\/(\d+)\//)?.[1];
    const meta = gid ? metaByGid.get(gid) : undefined;
    if (meta?.jpn && c.title && typeof c.title === "object") c.title.ja = meta.jpn;
    if (meta?.posted) c.posted_at = meta.posted;
    // The listing's own `uploader/<name>` link is kept when it had one (guest layout); the API is
    // authoritative otherwise. A rule over `uploader` is only answerable if this is populated.
    if (meta?.uploader) c.uploader = meta.uploader;
    // In bytes. A rule over `size` is only answerable once this is populated, and the host treats a
    // missing value as "cannot tell yet" rather than as "does not match".
    if (meta?.size !== undefined) c.size = meta.size;
  }
}

/** Parses the gallery rows of one listing page.
 *
 * Split per gallery row first, then read each row's own fields. Scanning the whole page for each
 * field separately would pair the Nth title with the Mth rating as soon as any row omits one, and
 * the host would then filter on another work's tags.
 *
 * E-Hentai renders two different listing layouts and the per-row container differs between them:
 * the guest/25-per-page layout uses `gl2c` rows, while the member/100-per-page grid wraps each work
 * in `gl1t`. Splitting only on `gl2c` matched nothing at all on the member page — which is how a
 * signed-in check could report zero candidates while the page on screen was full of works.
 */
function parsePage(html: string, seen: Set<string>): DiscoveredCandidateResult[] {
  const candidates: DiscoveredCandidateResult[] = [];
  const linkPattern = /https:\/\/e(?:-|x)hentai\.org\/g\/(\d+)\/([0-9a-f]{10})\//;
  const rowSeparator = html.includes('class="gl2c"') ? 'class="gl2c"' : 'class="gl1t"';

  for (const row of html.split(rowSeparator).slice(1)) {
    const link = row.match(linkPattern);
    if (!link) continue;
    const source = canonicalizeSource(`e-hentai.org/g/${link[1]}/${link[2]}`);
    // A listing links the same gallery from both its thumbnail and its title.
    if (seen.has(source)) continue;
    seen.add(source);

    const title = row.match(/class="[^"]*\bglink\b[^"]*"[^>]*>([^<]+)</)?.[1];
    const ratingStyle = row.match(/class="ir"\s+style="([^"]+)"/)?.[1];
    // `posted_<gid>` sits inside this same row and its id matches the row's own gid, so the pairing is
    // exact rather than positional. Declared on the candidate because that is what lets the host offer
    // "published more than N hours ago" — see `DiscoveredCandidate.posted_at`.
    const posted = row.match(/id="posted_\d+">([^<]+)</)?.[1];
    const category = row.match(/class="cn[^"]*"[^>]*>([^<]+)</)?.[1];
    // Best-effort from the row: the guest layout links `uploader/<name>` (and some variants carry
    // `?uploader=`), while the signed-in grid carries none. `addOriginalTitles` fills the gap from
    // the API, which has the uploader for every work either way — a missing one here is therefore
    // not final, and the host treats it as "unknown" rather than as "no uploader" regardless.
    const uploader = row.match(/uploader\/([^"'<>]+)/)?.[1] ?? row.match(/[?&]uploader=([^"'&<>]+)/)?.[1];
    const pages = row.match(/(\d+)\s*pages?/i)?.[1];
    // Namespaced exactly as the host's tag rules expect (`artist:foo`), so no translation is needed
    // on either side.
    const tags = [...row.matchAll(/class="gt"[^>]*title="([^"]+)"/g)].map((m) => m[1]);

    candidates.push({
      source,
      // `origin` is what the listing shows; `ja` is filled afterwards from the API, which is the only
      // place the original title exists.
      title: title ? { origin: decodeEntities(title) } : undefined,
      posted_at: posted?.trim(),
      rating: ratingStyle ? parseRating(ratingStyle) : undefined,
      tags,
      category: category?.trim(),
      uploader: uploader ? decodeURIComponent(uploader) : undefined,
      pages: pages ? Number(pages) : undefined,
      // Declared here, filled in below: the listing itself never shows a size, `addOriginalTitles`
      // reads it from `gdata`. The key has to be written where a candidate is built, because the
      // host's field introspection is static — it scans this file for the property name, so a field
      // only ever assigned from inside that helper would never be offered in the settings form.
      size: undefined,
    });
  }

  return candidates;
}

/** Unix seconds from a candidate's `posted_at`, whatever shape it currently has.
 *
 * The listing's own text has no timezone attached, so it is only a fallback for the rare case where
 * `gdata` did not answer; `gdata`'s value (a Unix-seconds string) is what the age check normally
 * sees. */
function postedSeconds(value: unknown): number | undefined {
  if (typeof value === "number") {
    return Number.isFinite(value) ? (value > 1e12 ? Math.floor(value / 1000) : value) : undefined;
  }
  if (value instanceof Date) return Math.floor(value.getTime() / 1000);
  const text = String(value ?? "").trim();
  if (!text) return undefined;
  if (/^\d+$/.test(text)) {
    const seconds = Number(text);
    return seconds > 1e12 ? Math.floor(seconds / 1000) : seconds;
  }
  const parsed = Date.parse(text.includes("T") ? text : text.replace(" ", "T"));
  return Number.isFinite(parsed) ? Math.floor(parsed / 1000) : undefined;
}

export async function discover(
  criteria: DiscoveryCriteriaArgs,
): Promise<DiscoveryResultShape> {
  const logger = legacyCompat.getLogger("EH Discovery", "plugins");
  const baseUrl = criteria.listing_url ?? searchUrl(criteria);
  // One page is not enough: a gallery edited to carry one of this subscription's tags enters the
  // result set at its *original* posting date, so it lands mid-list and is never visible from page
  // one. The host caps how far to go — a tag search here can exceed 49,000 results.
  const maxPages = Math.max(1, criteria.max_pages ?? 1);
  const maxCandidates =
    criteria.max_candidates && criteria.max_candidates > 0
      ? criteria.max_candidates
      : Number.MAX_SAFE_INTEGER;
  // Unix seconds. The host turns the user's "last N days" into this; the listing rows carry the
  // publication time themselves, so a page that is entirely outside the window ends the scan before
  // the next request rather than after reading the rest of a tag search.
  const cutoff = criteria.posted_after;

  const seen = new Set<string>();
  const candidates: DiscoveredCandidateResult[] = [];
  let next: string | undefined = criteria.start_after?.trim() || undefined;
  let pagesRead = 0;
  let degraded = false;
  let stop: DiscoveryResultShape["stop_reason"];

  for (let page = 0; page < maxPages; page++) {
    // E-Hentai pages by cursor (`next=<gid>`), not by page number: the id of the last gallery seen.
    const url = next ? `${baseUrl}${baseUrl.includes("?") ? "&" : "?"}next=${next}` : baseUrl;
    logger.debug(`discovering from ${url}`);

    let html: string;
    try {
      const response = await fetch(url, {
        headers: { "User-Agent": "Mozilla/5.0", ...authHeaders(criteria) },
      });
      if (!response.ok) {
        // A later page failing is not the same as never having looked: keep what earlier pages gave
        // rather than discarding a partial but real answer.
        if (pagesRead > 0) {
          logger.warn(`page ${page + 1} failed with ${response.status}; using earlier pages`);
          break;
        }
        return {
          error: {
            error_code: "Discovery request failed",
            data: { status: response.status },
          },
        };
      }
      html = await response.text();
    } catch (e) {
      if (pagesRead > 0) {
        logger.warn(`page ${page + 1} unreachable; using earlier pages`);
        break;
      }
      // Could not look at all — distinct from having looked and found nothing.
      return { error: { error_code: "Could not reach the source", data: { detail: String(e) } } };
    }

    if (looksLikeBotChallenge(html)) {
      // Reported even mid-run: a challenge page parses to zero candidates, and treating that as "no
      // more results" would silently truncate the listing.
      return {
        error: {
          error_code: "Blocked by a bot challenge",
          data: { detail: "Cloudflare answered with a challenge instead of a listing." },
        },
      };
    }

    pagesRead++;
    if (looksSignedOut(html)) degraded = true;

    const pageCandidates = parsePage(html, seen);
    await addOriginalTitles(pageCandidates, criteria, logger);

    // The window is a hard bound, not a hint: an entry older than it is not this subscription's to
    // act on, however cheaply it was listed.
    let inWindow = pageCandidates;
    if (cutoff !== undefined) {
      inWindow = pageCandidates.filter((c) => {
        const posted = postedSeconds(c.posted_at);
        return posted === undefined || posted >= cutoff;
      });
      const dated = pageCandidates.filter((c) => postedSeconds(c.posted_at) !== undefined);
      // A whole page outside the window means the next one is older still: stop rather than paging
      // through the rest of the result set for nothing.
      if (dated.length > 0 && inWindow.length === 0) {
        stop = "age";
        break;
      }
    }

    candidates.push(...inWindow);
    next = html.match(/[?&]next=(\d+)/)?.[1];
    if (!next) {
      stop = "end";
      break;
    }
    if (candidates.length >= maxCandidates) {
      stop = "cap";
      break;
    }
  }

  const trimmed = candidates.slice(0, maxCandidates);
  if (candidates.length > maxCandidates) stop = "cap";

  if (degraded) {
    logger.warn(
      `discovery ran without a usable login; reporting ${trimmed.length} candidate(s) as incomplete`,
    );
  }

  return {
    candidates: trimmed,
    degraded,
    next_cursor: next,
    pages_read: pagesRead,
    stop_reason: stop,
  };
}
