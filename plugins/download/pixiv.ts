// Converted from LANraragi/lib/LANraragi/Plugin/Download/Pixiv.pm via
// `lanrurugi-plugin-converter`, then hand-migrated to the `downloads[]`/`pluginOptions()`
// contract (specs/005-download-plugin-progress/contracts/plugin-download-protocol.md): this
// plugin no longer downloads/zips image bytes itself — it only resolves every page's real
// image URL (with the Referer header Pixiv's CDN requires) and hands them back as
// `downloads[]`; Rust does the actual byte-level fetch, respects per-domain concurrency/rate
// limits, and bundles all pages into one manga archive (`bundle_as_archive: true` below,
// matching the original Perl's own single-zip-per-artwork behavior).

declare global {
  interface PerlTransaction {
    req: { headers: { header(name: string, value: string): void } };
  }
  interface PerlUserAgent {
    cookie_jar: { add(cookie: { name: string; value: string; domain: string; path: string }): void };
    max_redirects(n: number): PerlUserAgent;
    transactor: { name(value: string): void };
    on(event: "start", handler: (ua: PerlUserAgent, tx: PerlTransaction) => void): void;
    get(url: string): Promise<{ result: PerlHttpResult }>;
    post(url: string, kind: "form" | "json", data: Record<string, string> | Record<string, unknown>): Promise<{ result: PerlHttpResult }>;
    cookies: { name: string; value: string; domain: string; path: string }[];
  }
  interface PerlHttpResult {
    body: string;
    code: number;
    readonly dom: PerlDomNode;
    readonly json: unknown;
  }
  interface PerlLogger {
    debug(msg: string): void;
    info(msg: string): void;
    warn(msg: string): void;
    error(msg: string): void;
  }
  interface PerlDomNode {
    text: string;
    attr(name: string): string | undefined;
    parent: PerlDomNode | undefined;
    at(selector: string): PerlDomNode | undefined;
    find(selector: string): PerlDomNode[] & { each<T>(fn: (node: PerlDomNode, index: number) => T): T[] };
    toString(): string;
  }
  // deno-lint-ignore no-var
  var perlCompat: {
    reverse<T>(list: readonly T[]): T[];
    chomp(s: string): string;
    sprintf(format: string, ...args: unknown[]): string;
    userAgent(): PerlUserAgent;
    getLogger(name: string, category: string): PerlLogger;
    htmlUnescape(s: string): string;
    parseHtml(markup: string, xml?: boolean): PerlDomNode;
    sleep(seconds: number): Promise<void>;
    getVersion(): { version: string; homepage: string };
    refType(x: unknown): string;
    trim(s: string | null | undefined): string;
    fileparse(path: string, suffixPattern?: unknown): [string, string, string];
    redis_decode(s: string): string;
  };
}

// Mirrors `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`'s `PluginErrorException` — defined
// locally (not imported) since a plugin file is loaded via a standalone `import()` with no
// relative-path relationship to the SDK file, and the dispatcher's catch block detects this by
// property shape (`error_code`/`data` on a thrown `Error`), not `instanceof`, for exactly that
// reason (see `dispatcher.ts`'s own comment on this). `error_code` is an i18n lookup key — write
// it as a natural, stable phrase that does not embed any dynamic value (that goes in `data`
// instead), so the same `error_code` translates regardless of which specific value triggered it.
class PluginErrorException extends Error {
  constructor(
    public error_code: string,
    public data?: Record<string, string | number>,
  ) {
    super(error_code);
  }
}

export function pluginInfo() {
  return {
    namespace: "pixivdl",
    type: "download" as const,
    parameters: [],
    // Only www.pixiv.net — this plugin's own ua.get() calls never touch the image CDN
    // (i.pximg.net) directly; the downloads[] URLs it returns for those images are fetched by
    // Rust itself (constitution Principle IV: no permission broader than what this process's own
    // code actually calls).
    declared_permissions: { net: ["www.pixiv.net"], read: false, write: false },
    name: "Pixiv Downloader",
    author: "thelastfantasy",
    description: "Downloads the given Pixiv artwork and adds it to LANraragi.\n            <br>\n            <br><i class='fa fa-exclamation-circle'></i> Pixiv enforces a rate limit on API requests, and may suspend/ban your account for overuse.",
    version: "0.1",
    login_from: "pixivlogin",
    url_pattern: "pixiv\\.net",
  };
}

export function pluginOptions() {
  return {
    domain_rules: [
      {
        pattern: "*.pixiv.net",
        max_concurrent: 2,
        description: "Limit simultaneous downloads from Pixiv's CDN",
      },
    ],
    bundle_as_archive: {
      default: true,
      description: "Combine all downloaded pages into a single manga archive instead of one archive per page",
    },
  };
}

interface DownloadRequest {
  url: string;
  method?: string;
  headers?: Record<string, string>;
  filename_hint?: string;
}

interface ExecDownloadInfo {
  user_agent: PerlUserAgent;
  user_agent_cookies?: { name: string; value: string; domain: string; path: string }[];
  url: string;
}

export async function execDownload(hostArgs: Record<string, unknown>) {
  const info = hostArgs as unknown as ExecDownloadInfo;
  const ua = perlCompat.userAgent();
  for (const c of info.user_agent_cookies ?? []) {
    ua.cookie_jar.add(c);
  }

  const logger = perlCompat.getLogger("Pixiv Downloader", "plugins");
  const url = info.url;
  const artworkId = extractArtworkId(url);
  if (!artworkId) {
    throw new PluginErrorException("Invalid Pixiv URL", { url });
  }
  logger.info(`Processing Pixiv artwork ID: ${artworkId}`);

  const referer = `https://www.pixiv.net/artworks/${artworkId}`;
  // Every real Perl call site sends this same static Referer on every API/CDN request for the
  // whole session — `on("start", ...)` sets it once here rather than threading it through every
  // individual `ua.get()` call (this SDK's `get()` takes no per-call headers, unlike Perl's own
  // `$ua->get($url => {Referer => $referer})` idiom).
  ua.on("start", (_ua, tx) => tx.req.headers.header("Referer", referer));

  const metadataResult = await fetchArtworkMetadata(ua, artworkId);
  if (metadataResult.error !== undefined) {
    throw new PluginErrorException(metadataResult.error.error_code, metadataResult.error.data);
  }
  const metadata = metadataResult.metadata as { body?: Record<string, unknown> } | undefined;
  if (!metadata?.body) {
    throw new PluginErrorException("Got invalid metadata response from artwork", { artworkId });
  }
  const artwork = metadata.body;
  const title = artwork.title;
  const pagesCount = artwork.pageCount as number;
  logger.debug(`Artwork title: ${title}, Pages: ${pagesCount}`);

  let downloads: DownloadRequest[];
  if (pagesCount === 1) {
    const imgUrl = (artwork.urls as Record<string, string> | undefined)?.original;
    if (!imgUrl) {
      throw new PluginErrorException("Could not find image URL in single-page artwork metadata", { artworkId });
    }
    downloads = [{ url: imgUrl, headers: { Referer: referer }, filename_hint: basename(imgUrl) }];
  } else if (pagesCount > 1) {
    downloads = await resolveMultiPageDownloads(ua, artworkId, referer);
  } else {
    throw new PluginErrorException("Invalid page count for artwork", { artworkId, pagesCount });
  }

  logger.info(`Resolved ${downloads.length} page URL(s) for artwork ${artworkId}`);
  return { downloads };
}

async function resolveMultiPageDownloads(
  ua: PerlUserAgent,
  artworkId: string,
  referer: string,
): Promise<DownloadRequest[]> {
  const logger = perlCompat.getLogger("Pixiv Downloader", "plugins");
  const pagesApiUrl = `https://www.pixiv.net/ajax/illust/${artworkId}/pages`;
  const pagesRes = (await ua.get(pagesApiUrl)).result;
  if (pagesRes.code < 200 || pagesRes.code >= 300) {
    throw new PluginErrorException("Failed to get pages metadata from multi-page artwork", {
      status: pagesRes.code,
    });
  }
  const pagesData = pagesRes.json as { body?: Array<{ urls: Record<string, string> }> } | undefined;
  if (!pagesData?.body) {
    throw new PluginErrorException("Invalid pages metadata response from multi-page artwork");
  }
  const pages = pagesData.body;
  const downloads: DownloadRequest[] = [];
  for (let i = 0; i < pages.length; i++) {
    const imgUrl = pages[i].urls.original;
    logger.info(`Resolved page ${i}: ${imgUrl}`);
    downloads.push({
      url: imgUrl,
      headers: { Referer: referer },
      filename_hint: perlCompat.sprintf("%03d_%s", i, basename(imgUrl)),
    });
  }
  return downloads;
}

async function fetchArtworkMetadata(
  ua: PerlUserAgent,
  artworkId: string,
): Promise<{ metadata?: unknown; error?: { error_code: string; data?: Record<string, string | number> } }> {
  const apiUrl = `https://www.pixiv.net/ajax/illust/${artworkId}`;
  const res = (await ua.get(apiUrl)).result;
  if (res.code < 200 || res.code >= 300) {
    return {
      error: { error_code: "Failed to fetch artwork metadata", data: { url: apiUrl, status: res.code } },
    };
  }
  try {
    return { metadata: res.json };
  } catch (caughtError) {
    return {
      error: { error_code: "Failed to parse metadata JSON response", data: { detail: String(caughtError) } },
    };
  }
}

function extractArtworkId(url: string): string | undefined {
  const match = url.match(/artworks\/([0-9]+)/);
  return match ? match[1] : undefined;
}

function basename(url: string): string {
  const path = new URL(url).pathname;
  const lastSlash = path.lastIndexOf("/");
  return lastSlash >= 0 ? path.slice(lastSlash + 1) : path;
}
