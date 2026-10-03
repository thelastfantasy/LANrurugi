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
      net: ["e-hentai.org", "exhentai.org"],
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

export async function discover(
  criteria: DiscoveryCriteriaArgs,
): Promise<DiscoveryResultShape> {
  const logger = legacyCompat.getLogger("EH Discovery", "plugins");
  const url = criteria.listing_url ?? searchUrl(criteria);
  logger.debug(`discovering from ${url}`);

  let html: string;
  try {
    const response = await fetch(url, {
      headers: { "User-Agent": "Mozilla/5.0" },
    });
    if (!response.ok) {
      return {
        error: {
          error_code: "Discovery request failed",
          data: { status: response.status },
        },
      };
    }
    html = await response.text();
  } catch (e) {
    // Could not look at all — distinct from having looked and found nothing.
    return { error: { error_code: "Could not reach the source", data: { detail: String(e) } } };
  }

  const degraded = looksSignedOut(html);

  // Gallery links carry the gid/token pair that identifies a work; everything else on a listing page
  // is navigation. Deduplicated because a listing links the same gallery from both its thumbnail and
  // its title.
  const seen = new Set<string>();
  const candidates: DiscoveredCandidateResult[] = [];
  const pattern = /https:\/\/e(?:-|x)hentai\.org\/g\/(\d+)\/([0-9a-f]{10})\//g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(html)) !== null) {
    const source = canonicalizeSource(`e-hentai.org/g/${match[1]}/${match[2]}`);
    if (seen.has(source)) continue;
    seen.add(source);
    candidates.push({ source });
  }

  if (degraded) {
    logger.warn(
      `discovery ran without a usable login; reporting ${candidates.length} candidate(s) as incomplete`,
    );
  }

  return { candidates, degraded };
}
