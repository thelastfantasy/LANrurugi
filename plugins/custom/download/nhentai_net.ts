export function pluginInfo() {
  return {
    namespace: "nhentai",
    type: "download" as const,
    url_pattern: "nhentai\\.net/g/",
    domain_match: ["nhentai.net"],
    login_from: "nhapiauth",
    parameters: [
      {
        name: "format",
        description:
          "Archive format to request from the nHentai API: zip (default), cbz, or torrent.",
        required: false,
        type: "string",
      },
    ],
    declared_permissions: { net: ["nhentai.net"], read: false, write: false },
    name: "nHentai",
    author: "LANrurugi AI Plugin Wizard",
    description:
      "Downloads nHentai galleries as zip/cbz/torrent archives using the official nHentai API v2. Requires the nHentai API Key login plugin (nhapiauth).",
    version: "1.0",
    generated_by_wizard: true,
  };
}

interface NHentaiDownloadHostArgs {
  url: string;
  category: string;
  customargs: string[];
  user_agent_cookies?: {
    name: string;
    value: string;
    domain?: string;
    path?: string;
  }[];
  user_agent_headers?: Record<string, string>;
}

const DOWNLOAD_FORMATS = ["zip", "cbz", "torrent"] as const;
type DownloadFormat = (typeof DOWNLOAD_FORMATS)[number];

const API_BASE = "https://nhentai.net/api/v2";

function pickFormat(raw: string | undefined): DownloadFormat {
  const value = (raw ?? "").trim().toLowerCase();
  return (DOWNLOAD_FORMATS as readonly string[]).includes(value)
    ? (value as DownloadFormat)
    : "zip";
}

function buildAuthHeaders(
  hostArgs: NHentaiDownloadHostArgs,
): Record<string, string> {
  const headers: Record<string, string> = {
    ...(hostArgs.user_agent_headers ?? {}),
  };
  if (!headers["User-Agent"]) {
    headers["User-Agent"] = "LANrurugi/1.0 (LANrurugi download plugin)";
  }
  if (!headers["Accept"]) {
    headers["Accept"] = "application/json";
  }
  // Defensive: if the companion login plugin authenticates via cookies instead
  // of a header, fold them into a Cookie header so the API call still works.
  const cookieHeader = (hostArgs.user_agent_cookies ?? [])
    .map((cookie) => `${cookie.name}=${cookie.value}`)
    .join("; ");
  if (cookieHeader) {
    headers["Cookie"] = cookieHeader;
  }
  return headers;
}

function errorForStatus(
  status: number,
): { error_code: string; data: { status: string } } {
  switch (status) {
    case 401:
      return {
        error_code: "nHentai API authentication failed",
        data: { status: String(status) },
      };
    case 404:
      return {
        error_code: "Gallery not found on nHentai",
        data: { status: String(status) },
      };
    case 422:
      return {
        error_code: "nHentai API rejected the request",
        data: { status: String(status) },
      };
    case 429:
      return {
        error_code: "nHentai API rate limit exceeded",
        data: { status: String(status) },
      };
    case 503:
      return {
        error_code: "nHentai downloads are temporarily disabled",
        data: { status: String(status) },
      };
    default:
      return {
        error_code: "nHentai API request failed",
        data: { status: String(status) },
      };
  }
}

function safeFilenamePart(value: string): string {
  return value
    .replace(/[\\/:*?"<>|\x00-\x1f]/g, "_")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 150);
}

export async function execDownload(hostArgs: NHentaiDownloadHostArgs) {
  const logger = legacyCompat.getLogger("nHentai Download", "plugins");
  const format = pickFormat(hostArgs.customargs[0]);

  const idMatch = /\/g\/(\d+)/.exec(hostArgs.url);
  if (!idMatch) {
    return {
      error: {
        error_code: "Not a valid nHentai gallery URL",
        data: { url: hostArgs.url },
      },
    };
  }
  const galleryId = idMatch[1];

  const headers = buildAuthHeaders(hostArgs);
  if (!headers["Authorization"]) {
    logger.warn(
      "No Authorization header available from the nhapiauth login plugin — the nHentai download API requires an API key",
    );
    return { error: { error_code: "Missing nHentai API credentials" } };
  }

  let response: Response;
  try {
    response = await fetch(
      `${API_BASE}/galleries/${galleryId}/download?format=${format}`,
      {
        method: "POST",
        headers,
      },
    );
  } catch (err) {
    logger.error(`Network error contacting nHentai API: ${err}`);
    return {
      error: {
        error_code: "Network error contacting nHentai API",
        data: { detail: String(err) },
      },
    };
  }

  const body = await response.text();
  if (!response.ok) {
    logger.warn(
      `nHentai API returned HTTP ${response.status} for gallery ${galleryId}`,
    );
    return { error: errorForStatus(response.status) };
  }

  let payload: { url?: string; expires_at?: number };
  try {
    payload = JSON.parse(body) as { url?: string; expires_at?: number };
  } catch {
    return {
      error: {
        error_code: "Could not parse nHentai API response",
        data: { status: String(response.status) },
      },
    };
  }
  if (!payload.url) {
    return { error: { error_code: "nHentai API returned no download URL" } };
  }

  // Optional nicety: fetch the gallery title for a human-friendly filename
  // hint. Failures here are non-fatal and never block the download itself.
  let title = "";
  try {
    const galleryResponse = await fetch(`${API_BASE}/galleries/${galleryId}`, {
      headers,
    });
    if (galleryResponse.ok) {
      const gallery = (await galleryResponse.json()) as {
        title?: { pretty?: string; english?: string };
      };
      title = gallery.title?.pretty || gallery.title?.english || "";
    }
  } catch {
    // ignore — filename hint stays just the gallery id
  }

  const extension = format === "torrent" ? "torrent" : format;
  const filenameHint = `${galleryId}${
    title ? ` - ${safeFilenamePart(title)}` : ""
  }.${extension}`;
  const downloadUrl = payload.url.startsWith("/")
    ? `https://nhentai.net${payload.url}`
    : payload.url;

  logger.info(
    `Resolved ${format} download URL for gallery ${galleryId} (expires ${
      payload.expires_at ?? "unknown"
    })`,
  );
  return {
    downloads: [{ url: downloadUrl, filename_hint: filenameHint }],
  };
}
